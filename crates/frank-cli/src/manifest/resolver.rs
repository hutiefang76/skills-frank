//! Skill 注册表: 已合并 skill 列表的查找与过滤。
//!
//! 上层命令 (install/list/enable/...) 拿到 [`Registry`] 之后只调用语义化方法,
//! 不直接操作 Vec, 避免到处写 `iter().find(|s| s.name == ...)`。

use crate::manifest::schema::Skill;

/// 已合并 + 排序的 skill 注册表。
#[derive(Debug)]
pub struct Registry {
    skills: Vec<Skill>,
}

impl Registry {
    /// 用一组合并好的 skills 构建注册表。
    #[must_use]
    pub fn new(skills: Vec<Skill>) -> Self {
        Self { skills }
    }

    /// 按 name 精确查找。
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// 全部 skills 切片。
    #[must_use]
    pub fn all(&self) -> &[Skill] {
        &self.skills
    }

    /// 按 profile 过滤 (skill 缺省 profile 视为 `personal`)。
    pub fn by_profile<'a>(&'a self, profile: &'a str) -> impl Iterator<Item = &'a Skill> + 'a {
        self.skills
            .iter()
            .filter(move |s| s.profile.as_deref().unwrap_or("personal") == profile)
    }

    /// 数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::schema::{Source, Visibility};

    fn s(name: &str, profile: Option<&str>) -> Skill {
        Skill {
            name: name.to_string(),
            description: String::new(),
            source: Source::Git {
                url: "x".to_string(),
                r#ref: "main".to_string(),
                subpath: None,
            },
            visibility: Visibility::Curated,
            auth: None,
            target_platforms: vec![],
            profile: profile.map(String::from),
            device_allowlist: vec![],
            require_network: Default::default(),
            dependencies: Default::default(),
            health_check: None,
            slash_command: None,
            mcp_server: None,
            metadata: Default::default(),
        }
    }

    #[test]
    fn find_returns_matching_skill() {
        let r = Registry::new(vec![s("a", None), s("b", None)]);
        assert!(r.find("a").is_some());
        assert!(r.find("c").is_none());
    }

    #[test]
    fn by_profile_filters_correctly() {
        let r = Registry::new(vec![
            s("a", None), // 默认 personal
            s("b", Some("personal")),
            s("c", Some("company")),
        ]);
        let p: Vec<_> = r.by_profile("personal").collect();
        assert_eq!(p.len(), 2);
        let c: Vec<_> = r.by_profile("company").collect();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "c");
    }
}
