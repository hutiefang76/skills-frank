//! 安装流程编排: device 校验 → git fetch → adapter 分发 → 失败回滚。
//!
//! 这是 [`crate::cli::install`] 的核心引擎; CLI 层只负责参数解析 + state 持久化,
//! 真正的"拉源码 + 分发到三平台"逻辑都在这里。
//!
//! # 失败回滚策略
//!
//! 多平台分发是顺序的; 任意一家 adapter 失败时, 已经成功的平台会反向调 `uninstall`
//! 兜底。兜底失败 (链接已残留) 只记一行 warn 不中断主错误传播 — 用户拿到的是
//! "原始失败原因", 残留留给 `frank doctor` 排查。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::adapter;
use crate::manifest::schema::{Platform, Skill, Source};

use super::git;

/// 一次安装的结果摘要, 返回给 CLI 层用于写 state.json + 显示。
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// 已 checkout 的源 commit SHA。`Source::Local` 时为 `"local"`。
    pub commit_sha: String,

    /// adapter 链接指向的源目录 (含 manifest subpath 拼接)。
    pub source_path: PathBuf,

    /// 实际成功安装的平台子集。
    pub platforms: Vec<Platform>,
}

/// 安装一个 skill 到其声明的全部 `target_platforms`。
///
/// 不写 state.json — 由调用方 (CLI 层) 决定状态记录与 UI 输出。
pub fn install_skill(skill: &Skill) -> Result<InstallOutcome> {
    check_device_allowlist(skill)?;

    let (commit_sha, source_root) = fetch_source(skill)?;
    let source_path = apply_subpath(&source_root, &skill.source);

    if !source_path.exists() {
        bail!(
            "source path {} does not exist after fetch (check manifest subpath)",
            source_path.display()
        );
    }

    let installed = distribute(skill, &source_path)?;

    Ok(InstallOutcome {
        commit_sha,
        source_path,
        platforms: installed,
    })
}

fn check_device_allowlist(skill: &Skill) -> Result<()> {
    if skill.device_allowlist.is_empty() {
        return Ok(());
    }
    let host = gethostname::gethostname().to_string_lossy().to_string();
    if !skill.device_allowlist.iter().any(|h| h == &host) {
        bail!(
            "device {host} not in allowlist {:?} for skill {}",
            skill.device_allowlist,
            skill.name
        );
    }
    Ok(())
}

fn fetch_source(skill: &Skill) -> Result<(String, PathBuf)> {
    match &skill.source {
        Source::Git { url, r#ref, .. } => {
            let result = git::fetch(url, r#ref)
                .with_context(|| format!("git fetch for skill {}", skill.name))?;
            Ok((result.commit_sha, result.repo_dir))
        }
        Source::Local { path } => {
            let p = PathBuf::from(path);
            if !p.exists() {
                bail!("local source {path} does not exist");
            }
            Ok(("local".to_string(), p))
        }
        Source::Upstream { parent } => {
            bail!("upstream source not yet implemented (parent={parent})");
        }
    }
}

fn apply_subpath(repo_root: &std::path::Path, source: &Source) -> PathBuf {
    match source {
        Source::Git {
            subpath: Some(sub), ..
        } => repo_root.join(sub),
        _ => repo_root.to_path_buf(),
    }
}

fn distribute(skill: &Skill, source_path: &std::path::Path) -> Result<Vec<Platform>> {
    let mut installed: Vec<Platform> = Vec::new();
    for &p in &skill.target_platforms {
        let adp = adapter::for_platform(p);
        match adp.install(&skill.name, source_path) {
            Ok(()) => {
                tracing::debug!(skill = %skill.name, platform = adp.name(), "installed");
                installed.push(p);
            }
            Err(e) => {
                // 回滚已成功的平台 (best effort)
                rollback(&skill.name, &installed);
                return Err(e.context(format!("install on platform {p:?} failed")));
            }
        }
    }
    Ok(installed)
}

fn rollback(name: &str, installed: &[Platform]) {
    for &p in installed {
        let adp = adapter::for_platform(p);
        if let Err(e) = adp.uninstall(name) {
            tracing::warn!(
                skill = %name,
                platform = adp.name(),
                error = %e,
                "rollback uninstall failed; manual cleanup may be needed"
            );
        }
    }
}

/// 卸载一个 skill (从全部 `platforms` 上移除链接)。
///
/// 单个 adapter 失败不中断后续 — 尽力卸完, 把所有错误聚合后再 bail。
pub fn uninstall_skill(name: &str, platforms: &[Platform]) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();
    for &p in platforms {
        let adp = adapter::for_platform(p);
        if let Err(e) = adp.uninstall(name) {
            errors.push(format!("{}: {e:#}", adp.name()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(
            "uninstall encountered errors:\n  - {}",
            errors.join("\n  - ")
        );
    }
}
