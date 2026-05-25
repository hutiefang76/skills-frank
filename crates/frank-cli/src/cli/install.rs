//! `frank install` 子命令: 安装一个 skill / MCP。
//!
//! # 流程 (详见 docs/DESIGN.md §4.2)
//!
//! 1. 解析 manifest, 定位 skill 元数据
//! 2. (推后 P1) health-check 前置 (依赖/网络/凭据/device-allowlist)
//! 3. (推后 P1) state snapshot
//! 4. [`crate::installer::install`] 拉源 + 分发到三平台
//! 5. 写 [`crate::state::State`]
//! 6. (推后 P2) sync-client 上报云端

use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use chrono::Utc;
use clap::Parser;

use crate::adapter;
use crate::installer::install as installer;
use crate::manifest::{
    parser as mparser,
    resolver::Registry,
    schema::{Skill, Source, Visibility},
};
use crate::scanner;
use crate::state::{SkillState, State};

/// `frank install` 参数。
#[derive(Parser, Debug)]
#[allow(clippy::struct_excessive_bools)] // CLI flag 集合, 拆 enum 反而难用
pub struct Args {
    /// 要安装的 skill 名称, 例如 `doris-ops` 或 `kdwl:vehicle-events`。
    pub name: Option<String>,

    /// 安装某个 profile 下的全部 skills。与 `name` 互斥。(P0 day5 待实现)
    #[arg(long)]
    pub all: bool,

    /// 指定 profile (例如 `personal` / `company`)。
    #[arg(long)]
    pub profile: Option<String>,

    /// 跳过 health-check (P0 health-check 尚未接入, 此 flag 当前是占位)。
    #[arg(long)]
    pub skip_health_check: bool,

    /// 即便已安装也强行覆盖 (会重新建链, 替换 state 记录)。
    #[arg(long)]
    pub force: bool,

    /// 已安装时拉取新 commit 但保留原始 `installed_at`。
    #[arg(long)]
    pub upgrade: bool,

    /// v0.7: 任意 git URL 装 (不通过 manifest). 例:
    /// `frank install --url https://github.com/foo/bar.git`
    /// 自动用 repo 名 (bar) 作 skill name, visibility=community.
    /// 含 subpath 用 `#subpath` 后缀: `--url https://.../repo.git#path/to/skill`.
    /// 含 branch 用 `?ref=xxx` query: `--url https://.../repo.git?ref=master`
    /// (跟 `--ref` flag 二选一; flag 优先).
    #[arg(long, value_name = "GIT_URL")]
    pub url: Option<String>,

    /// v0.9: 显式指定 git ref (branch/tag/sha) — 给 `--url` 配的, 不传默认 `main`.
    /// 修 v0.7 hardcoded main 的 bug (default-master 仓库装失败如 skills-nacos-ops).
    #[arg(long, value_name = "REF")]
    pub r#ref: Option<String>,
}

/// 执行 install 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "install invoked");

    if args.all {
        crate::log::ui::warn("`--all` not yet wired (P0 day5); pass a single skill name for now");
        return Ok(());
    }
    if args.skip_health_check {
        crate::log::ui::warn("`--skip-health-check` is a no-op (health-check not yet executed)");
    }

    // v0.7: --url 模式直接 synthesize Skill, 跳过 manifest 查找.
    let (name, skill_owned, skill_ref): (String, Option<crate::manifest::schema::Skill>, _);
    if let Some(url) = args.url.as_ref() {
        let s = synthesize_skill_from_url(url, args.name.as_deref(), args.r#ref.as_deref())?;
        name = s.name.clone();
        crate::log::ui::info(&format!(
            "--url 模式: 装 `{}` 从 {} (visibility=community)",
            s.name, url
        ));
        skill_owned = Some(s);
        skill_ref = skill_owned.as_ref().unwrap();
    } else {
        name = args.name.clone().ok_or_else(|| {
            anyhow!("提供 skill name (例 `frank install doris-ops`) 或 --url <git>")
        })?;

        // 1. 加载 manifest
        let manifests = mparser::discover()?;
        if manifests.is_empty() {
            bail!("no manifest found; expected manifest/public.yaml or ~/.frank/manifests/*.yaml");
        }
        let registry = Registry::new(mparser::merge(manifests));

        // 2. 找 skill
        let found = registry
            .find(&name)
            .ok_or_else(|| anyhow!("skill `{name}` not found in any manifest. 想装非内置: `frank install --url <git-url>`"))?;
        // registry 拥有 skill, 这里 clone 拿一份 owned 让分支统一
        skill_owned = Some(found.clone());
        skill_ref = skill_owned.as_ref().unwrap();
    }
    let skill = skill_ref;

    // 3. 容错: state 已存在 / 平台上已是 external 撞名 → 友好提示
    let mut state = State::load_default()?;
    let preexisting_installed_at = preflight_state_check(&state, &name, args.force, args.upgrade)?;
    preflight_external_check(&name, args.force)?;

    // 4. 跑安装
    let started = Instant::now();
    crate::log::ui::info(&format!("Installing `{name}`..."));
    let outcome = installer::install_skill(skill)?;
    let elapsed = started.elapsed();

    // 5. 写 state.json (upgrade 时保留 installed_at)
    let installed_at = preexisting_installed_at.unwrap_or_else(Utc::now);
    state.put(SkillState {
        name: skill.name.clone(),
        source_ref: outcome.commit_sha.clone(),
        source_path: outcome.source_path.clone(),
        platforms: outcome.platforms.clone(),
        installed_at,
        enabled: true,
        visibility: Some(skill.visibility),
    });
    state.save()?;

    // 6. 用户面输出 (短 sha 7 位即可识别)
    let short_sha: String = outcome.commit_sha.chars().take(7).collect();
    let verb = if preexisting_installed_at.is_some() {
        "reinstalled"
    } else {
        "installed"
    };
    crate::log::ui::success(&format!(
        "`{name}` {verb} ({} platform{}, {:.1}s) — sha {short_sha}",
        outcome.platforms.len(),
        if outcome.platforms.len() == 1 {
            ""
        } else {
            "s"
        },
        elapsed.as_secs_f64(),
    ));

    // v0.10.6 P2 D4: 静默生成 ~/.frank/claude-template.md (dormant artifact, Phase 3 hook 真用).
    // 不打印任何用户面消息 (是设计 — 详见 cli/claude_template.rs 模块文档).
    crate::cli::claude_template::ensure_claude_template_silent();

    Ok(())
}

/// 检查 state 已有该 skill 的处理: 没有 → Ok(None); 有 → 看 force/upgrade 决定。
fn preflight_state_check(
    state: &State,
    name: &str,
    force: bool,
    upgrade: bool,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let Some(existing) = state.get(name) else {
        return Ok(None);
    };
    let short_sha: String = existing.source_ref.chars().take(7).collect();
    let when = existing.installed_at.format("%Y-%m-%d %H:%M UTC");
    if upgrade {
        crate::log::ui::info(&format!(
            "upgrading `{name}` (current sha {short_sha} from {when}); installed_at preserved"
        ));
        return Ok(Some(existing.installed_at));
    }
    if force {
        crate::log::ui::warn(&format!(
            "`{name}` already installed (sha {short_sha} from {when}); --force overwriting"
        ));
        return Ok(None);
    }
    bail!(
        "`{name}` already installed (sha {short_sha} from {when}); use `--force` to reinstall or `--upgrade` to refresh"
    );
}

/// 检查平台目录是否存在同名 external 条目 (用户手工装的撞名), 友好提示走 import / force。
///
/// P2-1 fix (codex review): 早期版本用空 state 扫描, 导致 `frank install <name> --upgrade`
/// 把自己的 managed 链接误判为 external 然后 bail. 现在用真 state, 排除"已 managed 的 link
/// 指向预期 source_path"的健康情况.
fn preflight_external_check(name: &str, force: bool) -> Result<()> {
    let real_state = State::load_default()?;
    let mut platforms: Vec<crate::manifest::schema::Platform> = Vec::new();
    for &p in scanner::ALL_PLATFORMS {
        let scanned = scanner::scan_platform(p, &real_state)?;
        if scanned
            .iter()
            .any(|s| s.name == name && matches!(s.status, scanner::SkillStatus::External))
        {
            platforms.push(p);
        }
    }
    if platforms.is_empty() {
        return Ok(());
    }
    if force {
        // 把已有 external 条目清掉, 让 link_install 不再撞
        for &p in &platforms {
            let _ = adapter::for_platform(p).uninstall(name);
            // remove_link 只删 symlink; 若是真目录, 我们不动它 (link_install 会报错)
        }
        crate::log::ui::warn(&format!(
            "`{name}` already exists at {} platform(s); --force will overwrite (existing symlinks removed)",
            platforms.len()
        ));
        return Ok(());
    }
    bail!(
        "`{name}` already exists in {} platform skills dir as an external entry; run `frank import {name}` to manage it (or pass `--force` to overwrite)",
        platforms.len()
    );
}

/// `frank install --url <git>` 时把 URL 解析成临时 Skill struct (不写 manifest).
///
/// - URL 例 `https://github.com/foo/bar.git` → name="bar", subpath=None, ref="main"
/// - URL 例 `https://github.com/foo/bar.git#path/to/skill` → name="skill" (subpath 最后一段), subpath="path/to/skill"
/// - URL 例 `https://github.com/foo/bar.git?ref=master` → ref="master"  (v0.9)
/// - URL 例 `https://github.com/foo/bar.git?ref=master#path/to/skill` → 两者结合
/// - `override_ref` (来自 `--ref` flag) 优先于 URL query string. 都没则 `"main"`.
/// - 用户传 `name` 参数时覆盖自动推导的 name (`frank install --url ... my-name`)
/// - visibility 默认 community (用户开源 — 不算 frank-official 也不算 user-private)
fn synthesize_skill_from_url(
    url: &str,
    override_name: Option<&str>,
    override_ref: Option<&str>,
) -> Result<Skill> {
    // 先剥 #subpath, 再剥 ?ref=xxx — fragment 永远在 query 后
    let (url_no_frag, subpath) = url
        .split_once('#')
        .map_or((url.to_string(), None), |(u, p)| {
            (u.to_string(), Some(p.to_string()))
        });
    let (clean_url, ref_from_query) =
        url_no_frag
            .split_once('?')
            .map_or((url_no_frag.clone(), None), |(u, q)| {
                // 简单 query parse, 只认 ref=xxx (其它 query 暂时丢)
                let r = q
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("ref=").map(String::from));
                (u.to_string(), r)
            });
    let git_ref = override_ref
        .map(String::from)
        .or(ref_from_query)
        .unwrap_or_else(|| "main".to_string());
    // 推导 name: subpath 最后一段, 没就 url repo 名 (去掉 .git)
    let auto_name = subpath
        .as_deref()
        .and_then(|p| p.rsplit('/').next())
        .map(String::from)
        .or_else(|| {
            clean_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .map(|s| s.trim_end_matches(".git").to_string())
        })
        .ok_or_else(|| anyhow!("can't infer name from URL `{url}`"))?;
    let name = override_name.map_or(auto_name, String::from);
    if name.is_empty() {
        bail!("skill name 推导为空, 传 `frank install --url <url> <name>` 显式指定");
    }
    Ok(Skill {
        name: name.clone(),
        description: format!("Ad-hoc install via --url {url}"),
        source: Source::Git {
            url: clean_url,
            r#ref: git_ref,
            subpath,
        },
        visibility: Visibility::Community,
        auth: None,
        target_platforms: vec![
            crate::manifest::schema::Platform::Claude,
            crate::manifest::schema::Platform::Codex,
        ],
        profile: None,
        device_allowlist: vec![],
        require_network: crate::manifest::schema::NetworkReq::Internet,
        dependencies: crate::manifest::schema::Dependencies::default(),
        health_check: None,
        slash_command: None,
        mcp_server: None,
        metadata: std::collections::HashMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::schema::Source;

    fn extract_ref_and_subpath(s: &Skill) -> (String, Option<String>) {
        let Source::Git { r#ref, subpath, .. } = &s.source else {
            panic!("expected Source::Git")
        };
        (r#ref.clone(), subpath.clone())
    }

    #[test]
    fn url_default_ref_is_main() {
        let s = synthesize_skill_from_url("https://github.com/foo/bar.git", None, None).unwrap();
        let (r, sp) = extract_ref_and_subpath(&s);
        assert_eq!(r, "main");
        assert_eq!(sp, None);
        assert_eq!(s.name, "bar");
    }

    #[test]
    fn flag_ref_overrides_default() {
        let s = synthesize_skill_from_url("https://github.com/foo/bar.git", None, Some("master"))
            .unwrap();
        assert_eq!(extract_ref_and_subpath(&s).0, "master");
    }

    #[test]
    fn url_query_ref_parsed() {
        let s = synthesize_skill_from_url("https://github.com/foo/bar.git?ref=dev", None, None)
            .unwrap();
        assert_eq!(extract_ref_and_subpath(&s).0, "dev");
    }

    #[test]
    fn flag_ref_wins_over_query_ref() {
        let s =
            synthesize_skill_from_url("https://github.com/foo/bar.git?ref=dev", None, Some("prod"))
                .unwrap();
        assert_eq!(extract_ref_and_subpath(&s).0, "prod");
    }

    #[test]
    fn url_subpath_fragment_still_works() {
        let s = synthesize_skill_from_url("https://github.com/foo/bar.git#sub/path", None, None)
            .unwrap();
        let (r, sp) = extract_ref_and_subpath(&s);
        assert_eq!(r, "main");
        assert_eq!(sp.as_deref(), Some("sub/path"));
        assert_eq!(s.name, "path"); // 最后一段
    }

    #[test]
    fn url_query_plus_subpath_fragment_combined() {
        let s = synthesize_skill_from_url(
            "https://github.com/foo/bar.git?ref=master#some/sub",
            None,
            None,
        )
        .unwrap();
        let (r, sp) = extract_ref_and_subpath(&s);
        assert_eq!(r, "master");
        assert_eq!(sp.as_deref(), Some("some/sub"));
    }

    #[test]
    fn override_name_wins() {
        let s = synthesize_skill_from_url("https://github.com/foo/bar.git", Some("my-alias"), None)
            .unwrap();
        assert_eq!(s.name, "my-alias");
    }
}
