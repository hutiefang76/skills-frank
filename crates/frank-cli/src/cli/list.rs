//! `frank list` 子命令: 显示已知的 skills 表格。

use std::collections::HashSet;

use anyhow::Result;
use clap::Parser;
use tabled::{Table, Tabled};

use crate::manifest::{parser, resolver::Registry, schema::Skill};
use crate::state::State;

/// `frank list` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 仅列出指定 profile 的 skills (例如 `personal` / `company`)。
    #[arg(long)]
    pub profile: Option<String>,

    /// 仅列出已安装的 (state.json 中存在记录)。
    #[arg(long)]
    pub installed: bool,
}

/// 表格行结构 (tabled derive)。
#[derive(Tabled)]
struct Row {
    /// skill 名称。
    name: String,

    /// 可见性 (public / own-public / private)。
    visibility: String,

    /// 归属 profile。
    profile: String,

    /// 安装状态: `-` (未装) / `enabled` / `disabled`。
    status: String,

    /// 一句话说明 (太长会截断)。
    description: String,
}

impl Row {
    fn from_skill(s: &Skill, status: &str) -> Self {
        Self {
            name: s.name.clone(),
            visibility: format!("{:?}", s.visibility)
                .to_lowercase()
                .replace("ownpublic", "own-public"),
            profile: s.profile.clone().unwrap_or_else(|| "personal".to_string()),
            status: status.to_string(),
            description: truncate(&s.description, 60),
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

/// 执行 list 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "list invoked");

    let manifests = parser::discover()?;
    if manifests.is_empty() {
        crate::log::ui::warn(
            "no manifest found; expected `<repo>/manifest/public.yaml` or `~/.frank/manifests/*.yaml`",
        );
        return Ok(());
    }

    let registry = Registry::new(parser::merge(manifests));

    // 同时加载 state, 给每行打 install 状态。state 失败不让 list 整体挂 — 退化成空表。
    let state = State::load_default().unwrap_or_else(|e| {
        tracing::debug!(error = %e, "load state failed; treating as empty");
        State::load(std::env::temp_dir().join("frank-nonexistent-state.json"))
            .expect("loading nonexistent file always succeeds")
    });

    let installed_names: HashSet<&str> = state.iter().map(|s| s.name.as_str()).collect();

    let candidate: Box<dyn Iterator<Item = &Skill>> = if let Some(p) = &args.profile {
        Box::new(registry.by_profile(p))
    } else {
        Box::new(registry.all().iter())
    };

    let rows: Vec<Row> = candidate
        .filter_map(|s| {
            let status =
                state.get(&s.name).map_or(
                    "-",
                    |st| {
                        if st.enabled {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    },
                );
            if args.installed && !installed_names.contains(s.name.as_str()) {
                return None;
            }
            Some(Row::from_skill(s, status))
        })
        .collect();

    if rows.is_empty() {
        crate::log::ui::warn("no skill matched filter");
        return Ok(());
    }

    crate::log::ui::section(&format!("Skills ({} total)", rows.len()));
    println!("{}", Table::new(rows));
    Ok(())
}
