//! Manifest 数据结构定义 (serde 派生 YAML 序列化)。
//!
//! 与 `docs/DESIGN.md §7.1` 中的 schema 严格对应; 任何字段变更都要同步设计文档。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 顶层 manifest 文件结构 (对应一个 .yaml 文件)。
///
/// 例如 `manifest/public.yaml` 或 `~/.frank/manifests/company-kdwl.yaml`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// schema 版本, 用于向后兼容判断。当前唯一合法值: 1。
    pub schema_version: u32,

    /// 该 manifest 默认归属的 profile (例如 `personal` / `company`)。
    /// 单个 skill 可在 Skill::profile 里覆盖。
    #[serde(default = "default_profile")]
    pub profile: String,

    /// skill / MCP 条目列表。
    #[serde(default)]
    pub skills: Vec<Skill>,
}

/// 一个 skill 或 MCP server 的完整描述。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// 唯一标识。命名空间用 `:` 分隔 (例如 `kdwl:vehicle-events`)。
    pub name: String,

    /// 一句话说明, 显示在 `frank list` 表格里。
    #[serde(default)]
    pub description: String,

    /// 源代码位置。
    pub source: Source,

    /// 可见性 / 权限分档。详见 docs/DESIGN.md §6.2.3。
    pub visibility: Visibility,

    /// 鉴权配置 (可选; private 时强烈推荐)。
    #[serde(default)]
    pub auth: Option<Auth>,

    /// 目标平台。默认全部三家。
    #[serde(default = "default_platforms")]
    pub target_platforms: Vec<Platform>,

    /// 归属 profile, 覆盖文件级别。
    #[serde(default)]
    pub profile: Option<String>,

    /// 设备 allowlist (hostname); 不在列表的设备拒绝安装。
    #[serde(default)]
    pub device_allowlist: Vec<String>,

    /// 安装/运行前置网络要求。
    #[serde(default)]
    pub require_network: NetworkReq,

    /// 依赖 (python pkg / 系统 bin / 其他 MCP)。
    #[serde(default)]
    pub dependencies: Dependencies,

    /// 健康检查配置 (可选)。
    #[serde(default)]
    pub health_check: Option<HealthCheck>,

    /// Slash command 注册 (可选)。
    #[serde(default)]
    pub slash_command: Option<SlashCommand>,

    /// MCP server 配置 (如果这是 MCP 而非 skill)。
    #[serde(default)]
    pub mcp_server: Option<McpServer>,

    /// 元数据 (作者/版本/license 等)。
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// skill 源代码位置, 三种类型互斥。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    /// git 仓库 (最常见)。
    Git {
        /// 仓库 URL (SSH 或 HTTPS)。
        url: String,
        /// 分支 / tag / commit SHA, 默认 `main`。
        #[serde(default = "default_ref")]
        r#ref: String,
        /// 多 skill 单仓时指定子目录, 例如 `internal/vehicle-events`。
        #[serde(default)]
        subpath: Option<String>,
    },
    /// 本地目录 (开发期用)。
    Local {
        /// 绝对路径。
        path: String,
    },
    /// 引用其他 manifest 中已声明的 skill (复用)。
    Upstream {
        /// 上游 skill 的 name。
        parent: String,
    },
    /// MCP server (v0.4 新增) — 不走 git/symlink, 走平台 MCP 配置注入。
    ///
    /// install 时:
    /// - claude: 改 `~/.claude.json` `mcpServers.<name> = { command, args }`
    /// - codex: 追加 `~/.codex/config.toml` `[mcp_servers.<name>]` (v0.5+)
    /// - opencode: 暂不支持 MCP
    ///
    /// uninstall 反向删那一项. 不动其他 mcpServers 条目, 不动 manifest 外字段.
    Mcp {
        /// 启动命令 (例 `npx` / `uvx` / 二进制路径)。
        command: String,
        /// 命令参数 (例 `["-y", "@modelcontextprotocol/server-time"]`)。
        #[serde(default)]
        args: Vec<String>,
        /// 环境变量 (可选, 例 API key, 通常引 keychain)。
        #[serde(default)]
        env: std::collections::HashMap<String, String>,
    },
}

/// skill 可见性 / 权限分档 (v0.6.2 直观命名)。
///
/// # 两层 5 档
///
/// **Layer 1: frank 内置** (装 frank 默认带, 项目方维护)
/// - `frank-official`    — frank 出品: 项目方自家维护的开源 skills (例 nacos-ops, streampark-ops)
/// - `frank-recommended` — frank 推荐: 项目方精选的 upstream / 第三方 skills (例 skill-creator)
///
/// **Layer 2: 用户自定义** (用户自己加在 `~/.frank/manifests/`, 跟项目方无关)
/// - `community` — 用户开源的 skills (任意公开 git URL)
/// - `team`      — 用户所在团队 / 公司的 skills (严禁混入本仓!)
/// - `private`   — 用户私有的 skills (仅本机 / 个人凭据)
///
/// # 历史命名兼容 (#[serde(alias)] 全部支持)
///
/// - v0.6.0 `official` / `curated` (太抽象, v0.6.2 废)
/// - v0.5   `frank-own` / `user-public` / `user-company` / `user-private`
/// - v0.1   `public` / `own-public` / `private`
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    /// frank 项目方自研开源 — 装 frank 默认带, 维护者是项目方 (例: nacos-ops, streampark-ops).
    #[serde(alias = "official", alias = "frank-own", alias = "own-public")]
    FrankOfficial,

    /// frank 项目方精选的 upstream / 第三方 skill — 默认列在 builtin, 一键装 (例: skill-creator).
    #[serde(alias = "curated", alias = "public")]
    FrankRecommended,

    /// 用户自己开源的 skills — 任意公开 git URL, 无凭据.
    #[serde(alias = "user-public")]
    Community,

    /// 用户所在**团队/公司** skills — 跟 frank 项目方无关, 严禁混入本仓! 用 ~/.frank/manifests/.
    #[serde(alias = "user-company")]
    Team,

    /// 用户自己私有的 skills — 仅本机 / 个人凭据访问.
    #[serde(alias = "user-private")]
    Private,
}

impl Visibility {
    /// 是否属于 frank 内置 (项目方维护, 装 frank 默认有 — frank-official + frank-recommended)。
    #[must_use]
    pub fn is_frank_builtin(self) -> bool {
        matches!(self, Self::FrankOfficial | Self::FrankRecommended)
    }

    /// 是否属于用户自定义 (用户自己 manifest 加的)。
    #[must_use]
    pub fn is_user_custom(self) -> bool {
        !self.is_frank_builtin()
    }
}

/// 鉴权方式。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    /// 方法: none / ssh-key / github-pat / oauth。
    pub method: AuthMethod,

    /// 凭据指针 (keychain key 名), 绝不存明文。
    #[serde(default)]
    pub key_ref: Option<String>,

    /// 是否要求 MFA (公司 skills 推荐 true)。
    #[serde(default)]
    pub require_mfa: bool,
}

/// 鉴权方式枚举。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    /// 不需要 (公共 HTTPS clone)。
    None,
    /// SSH key (走系统 ~/.ssh)。
    SshKey,
    /// GitHub Personal Access Token (走 keychain)。
    GithubPat,
    /// OAuth (P4 才考虑)。
    Oauth,
}

/// 目标平台枚举。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// Claude Code CLI。
    Claude,
    /// codex CLI。
    Codex,
    /// opencode CLI。
    Opencode,
    /// v0.14: Gemini CLI (`~/.gemini/settings.json`).
    ///
    /// MCP 写入端点已实 (installer/mcp.rs install_gemini); skill (symlink) 装支持
    /// `~/.gemini/skills/` — Gemini CLI 真支持 skill 目录就启用, 否则当成 MCP-only.
    Gemini,
}

/// 网络前置要求。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkReq {
    /// 无要求。
    #[default]
    None,
    /// 需要公网。
    Internet,
    /// 需要公司 OpenVPN。
    Vpn,
    /// 需要公司内网 (办公地直连或 IOA)。
    CorpNet,
}

/// 依赖列表 (运行 skill 所需的外部资源)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Dependencies {
    /// Python 包列表, 例如 `["pymongo>=4.0"]`。
    #[serde(default)]
    pub python: Vec<String>,

    /// 系统二进制依赖, 例如 `["git", "openvpn"]`。
    #[serde(default)]
    pub system: Vec<String>,

    /// 依赖的其他 MCP server 名字。
    #[serde(default)]
    pub mcp: Vec<String>,
}

/// 健康检查配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheck {
    /// 探针命令, 退出码 0 = 健康。
    pub cmd: String,

    /// 超时秒数, 默认 10。
    #[serde(default = "default_health_timeout")]
    pub timeout_seconds: u32,

    /// 是否在安装前跑一次。
    #[serde(default)]
    pub run_before_install: bool,
}

/// Slash command 注册配置 (claude / codex 支持)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommand {
    /// 是否启用。
    pub enabled: bool,

    /// 命令名 (默认与 skill name 相同)。
    #[serde(default)]
    pub name: Option<String>,

    /// 要注册的目标平台子集。
    #[serde(default = "default_slash_platforms")]
    pub platforms: Vec<Platform>,
}

/// MCP server 启动配置 (仅当这条记录是 MCP 而非 skill)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    /// 启动命令, 例如 `["node", "server.js"]`。
    pub command: Vec<String>,

    /// 环境变量。
    #[serde(default)]
    pub env: HashMap<String, String>,
}

// ---- serde 默认值辅助函数 ----

fn default_profile() -> String {
    "personal".to_string()
}

fn default_ref() -> String {
    "main".to_string()
}

fn default_platforms() -> Vec<Platform> {
    // v0.6 收缩到稳定的两家. opencode 仍然是合法值, 用户在 manifest 显式写
    // `target_platforms: [opencode]` 仍然装. 但默认不装 — opencode CLI 接口变动频繁,
    // skill 装上去经常水土不服 (实测有过 SKILL.md YAML 解析炸的 case).
    vec![Platform::Claude, Platform::Codex]
}

fn default_slash_platforms() -> Vec<Platform> {
    vec![Platform::Claude, Platform::Codex]
}

fn default_health_timeout() -> u32 {
    10
}

// ---- 单元测试: 验证 YAML 解析正确性 ----

#[cfg(test)]
mod tests {
    use super::*;

    /// v0.2 新 visibility 值 (frank-own / frank-recommended / user-public / user-company / user-private)。
    #[test]
    fn parses_minimal_skill_v02() {
        let yaml = r"
schema_version: 1
skills:
  - name: doris-ops
    source:
      type: git
      url: https://github.com/hutiefang76/skills-doris-ops.git
    visibility: frank-own
";
        let m: Manifest = serde_yml::from_str(yaml).expect("parse minimal manifest");
        assert_eq!(m.skills.len(), 1);
        assert!(matches!(m.skills[0].visibility, Visibility::FrankOfficial));
        assert!(m.skills[0].visibility.is_frank_builtin());
    }

    /// v0.1 / v0.5 / v0.6.0 老 manifest (`own-public` / `public` / `private` / `official` / `curated` / `frank-own`)
    /// 仍能 load — alias 兼容。
    #[test]
    fn parses_v01_aliases() {
        for (alias, expect) in [
            ("own-public", Visibility::FrankOfficial),
            ("public", Visibility::FrankRecommended),
            ("private", Visibility::Private),
            ("official", Visibility::FrankOfficial),
            ("curated", Visibility::FrankRecommended),
            ("frank-own", Visibility::FrankOfficial),
        ] {
            let yaml = format!(
                "schema_version: 1\nskills:\n  - name: x\n    source: {{ type: git, url: 'https://example/x.git' }}\n    visibility: {alias}\n"
            );
            let m: Manifest = serde_yml::from_str(&yaml).unwrap();
            assert_eq!(
                m.skills[0].visibility, expect,
                "alias {alias} 应该映射到 {expect:?}"
            );
        }
    }

    /// 用户自己公司的 skill (`user-company`) — 跟 frank 项目方无关。
    #[test]
    fn parses_user_company_skill() {
        let yaml = r"
schema_version: 1
profile: company
skills:
  - name: kdwl:vehicle-events
    source:
      type: git
      url: git@github.com:hutiefang76/skills-kdwl.git
      ref: main
      subpath: internal/vehicle-events
    visibility: user-company
    auth:
      method: ssh-key
      key_ref: id_ed25519_personal
    require_network: vpn
    device_allowlist:
      - ATHENA-LAPTOP
";
        let m: Manifest = serde_yml::from_str(yaml).expect("parse user-company skill");
        let s = &m.skills[0];
        assert!(matches!(s.visibility, Visibility::Team));
        assert!(s.visibility.is_user_custom());
        assert_eq!(s.require_network, NetworkReq::Vpn);
    }
}
