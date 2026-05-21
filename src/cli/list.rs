//! `frank list` 子命令: 显示已知的 skills 表格。

use anyhow::Result;
use clap::Parser;
use tabled::{Table, Tabled};

use crate::manifest::{parser, resolver::Registry, schema::Skill};

/// `frank list` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 仅列出指定 profile 的 skills (例如 `personal` / `company`)。
    #[arg(long)]
    pub profile: Option<String>,

    /// 仅列出已安装的 (P0 后续 day: 待 state 模块完成)。
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

    /// 一句话说明 (太长会截断)。
    description: String,
}

impl Row {
    fn from_skill(s: &Skill) -> Self {
        Self {
            name: s.name.clone(),
            visibility: format!("{:?}", s.visibility).to_lowercase().replace("ownpublic", "own-public"),
            profile: s.profile.clone().unwrap_or_else(|| "personal".to_string()),
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

    let rows: Vec<Row> = if let Some(p) = &args.profile {
        registry.by_profile(p).map(Row::from_skill).collect()
    } else {
        registry.all().iter().map(Row::from_skill).collect()
    };

    if rows.is_empty() {
        crate::log::ui::warn("no skill matched filter");
        return Ok(());
    }

    crate::log::ui::section(&format!("Skills ({} total)", rows.len()));
    println!("{}", Table::new(rows));

    if args.installed {
        crate::log::ui::warn("--installed flag not yet wired up (P0 day3-4 待 state 模块)");
    }
    Ok(())
}
