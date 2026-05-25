//! 跨平台机器指纹采集 (v0.13.0).
//!
//! # 用途
//!
//! 服务端要把 token 跟具体设备绑定 (防一台机器注册无数 tenant), 但又不愿在
//! 协议里塞太多敏感字段, 所以走 "客户端采指纹 → server `sha256` → 当 `machine_code`" 的
//! 套路 (类似 mac0/mem0 那一派做法).
//!
//! 客户端职责 (本模块):
//! 1. [`collect_fingerprint`] 拿到一个 [`MachineFingerprint`].
//! 2. [`fingerprint_to_canonical_json`] 序列化成**确定性** JSON (字段顺序固定).
//! 3. POST 给 frank-sync-agent 的 `/auth/register` (调用方负责, 不在本模块).
//!
//! 服务端职责 (不在本仓):
//! - `sha256(canonical_json)` → `machine_code` (32 字节 hex)
//! - `(tenant, machine_code)` 表里去重: 同 machine_code 已存在 token 则复用.
//!
//! # 调用方
//!
//! - `cli::tenant` (待 v0.13.0 整合时接入) — 第一次 `frank login` 时上送.
//!
//! # 跨平台
//!
//! macOS / Linux / Windows 全支持. 任何字段失败都填默认值 (空 / `"unknown"`),
//! **不会 panic**, 让上层流程能继续走 — 哪怕指纹质量退化, 服务端拿到的 hash 仍
//! 是稳定的同一份字符串.
//!
//! # 设计要点
//!
//! - **`MAC` 过滤** — 跳虚拟网卡 (docker / vbox / vmware / tun / lo) 通过名字查询;
//!   名字查不到时按 `OUI` 前缀 + locally-administered bit 兜底.
//! - **确定性 JSON** — 用 `serde_json::to_string`, 因为 `MachineFingerprint` 是
//!   `struct` (字段顺序固定), `Vec` 已排序, 不含 `HashMap` / 无 `f64`, 同 input 必定
//!   同 output.
//! - **稳定字段** — 都是机器属性 (不含 PID / 时间戳 / 进程内随机数). 同一机器
//!   隔天跑也是同 hash; 只有换了网卡或重装系统才会变.

use std::collections::BTreeSet;

use mac_address::{name_by_mac_address, MacAddressIterator};
use serde::{Deserialize, Serialize};
use sysinfo::System;

/// Frank 机器指纹 — 用于服务端派生 `machine_code` (`sha256` hash).
///
/// 字段稳定 + 公开 (用户能 audit 这里有什么). 不含敏感数据 (无序列号 / 无
/// SSH key / 无邮箱). 客户端调 [`collect_fingerprint`] 拿到, 序列化后发给
/// server, server hash 完丢弃明文.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineFingerprint {
    /// hostname.
    pub hostname: String,
    /// MAC 地址列表 (去重 + 排序), 跳过虚拟网卡 (vboxnet0 / docker0 / 类似).
    pub mac_addresses: Vec<String>,
    /// OS 类型 ("macos" / "linux" / "windows" / "unknown").
    pub os: String,
    /// OS 版本 (e.g. "14.5.0" for macOS Sonoma).
    pub os_version: String,
    /// CPU 厂商 + 型号 (e.g. "Apple M2 Pro").
    pub cpu_brand: String,
    /// CPU 核心数 (逻辑核 sum, 简单粗).
    pub cpu_cores: u32,
    /// 总内存 MB.
    pub total_memory_mb: u64,
}

impl Default for MachineFingerprint {
    fn default() -> Self {
        Self {
            hostname: String::from("unknown"),
            mac_addresses: Vec::new(),
            os: String::from("unknown"),
            os_version: String::from("unknown"),
            cpu_brand: String::from("unknown"),
            cpu_cores: 0,
            total_memory_mb: 0,
        }
    }
}

/// 采集当前机器的指纹. 跨平台. 失败字段填默认值 ("unknown" / 空 vec / 0).
///
/// 全过程**无 panic** — `sysinfo` 拿不到某项就跳过, `mac_address` 出错就空列表.
/// 上层应该总能拿到一个可用的 [`MachineFingerprint`].
///
/// # Examples
///
/// ```no_run
/// let fp = frank::machine_id::collect_fingerprint();
/// println!("{} {}", fp.os, fp.cpu_brand);
/// ```
#[must_use]
pub fn collect_fingerprint() -> MachineFingerprint {
    let mut sys = System::new_all();
    sys.refresh_all();

    let hostname = System::host_name().unwrap_or_else(|| String::from("unknown"));
    let os = normalize_os_name(&System::name().unwrap_or_default());
    let os_version = System::os_version().unwrap_or_else(|| String::from("unknown"));

    let (cpu_brand, cpu_cores) = collect_cpu(&sys);
    let total_memory_mb = sys.total_memory() / 1_048_576; // bytes → MB (1024 * 1024)

    let mac_addresses = collect_physical_macs();

    MachineFingerprint {
        hostname,
        mac_addresses,
        os,
        os_version,
        cpu_brand,
        cpu_cores,
        total_memory_mb,
    }
}

/// fingerprint → 稳定 JSON 字符串 (用于 server `sha256`).
///
/// **确定性**: 同 input → 必定同 output. 因为
/// 1. `MachineFingerprint` 是 `struct` (字段按 `Serialize` derive 顺序输出).
/// 2. `mac_addresses` 是 `Vec<String>` 已在 [`collect_fingerprint`] 内 sort + dedup.
/// 3. 不含 `HashMap` / `f64` / 系统时间, 无源于运行时的不确定性.
///
/// 失败时返回 `{}` (理论上 `MachineFingerprint` 不会序列化失败, 兜底防御).
#[must_use]
pub fn fingerprint_to_canonical_json(fp: &MachineFingerprint) -> String {
    serde_json::to_string(fp).unwrap_or_else(|_| String::from("{}"))
}

// ---------- 内部 helpers ----------

/// 把 `sysinfo::System::name()` 返回的 "Darwin" / "Linux" / "Windows" / 各发行版名
/// 规范化为 frank 内部用的 short tag.
fn normalize_os_name(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("darwin") || lower.contains("mac") {
        String::from("macos")
    } else if lower.contains("windows") {
        String::from("windows")
    } else if lower.is_empty() {
        String::from("unknown")
    } else {
        // Linux / *BSD / 其他 — 统一按 lowercase short tag, 大部分是 "linux"
        String::from("linux")
    }
}

/// 拿 CPU 信息: 用第 0 个核的 brand (M-series / Intel 同机器同型号), 核心数取 `cpus().len()`.
fn collect_cpu(sys: &System) -> (String, u32) {
    let cpus = sys.cpus();
    if cpus.is_empty() {
        return (String::from("unknown"), 0);
    }
    let brand = cpus[0].brand().trim().to_string();
    let brand = if brand.is_empty() {
        String::from("unknown")
    } else {
        brand
    };
    let cores = u32::try_from(cpus.len()).unwrap_or(u32::MAX);
    (brand, cores)
}

/// 枚举所有网卡 MAC, 过滤虚拟网卡, 去重 + 排序后返回.
fn collect_physical_macs() -> Vec<String> {
    let Ok(iter) = MacAddressIterator::new() else {
        return Vec::new();
    };
    let mut set: BTreeSet<String> = BTreeSet::new();
    for mac in iter {
        let bytes = mac.bytes();
        let s = mac.to_string().to_lowercase();

        // 全零 MAC 直接跳
        if bytes == [0u8; 6] {
            continue;
        }

        // 按名字过滤虚拟网卡 — 名字查得到就按名字, 查不到走 OUI 兜底
        match name_by_mac_address(&mac) {
            Ok(Some(name)) => {
                if !is_physical_iface_name(&name) {
                    continue;
                }
            }
            _ => {
                // 名字查不到 — 走 MAC 前缀兜底
                if !is_physical_mac(&s) {
                    continue;
                }
            }
        }

        set.insert(s);
    }
    set.into_iter().collect()
}

/// 网卡名是物理网卡? (true = 留, false = 过滤掉)
///
/// 关键字匹配 — 已知虚拟网卡都有规律命名 (docker0 / vboxnet0 / vmnet1 / tun0 / lo0).
fn is_physical_iface_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    const VIRTUAL_PATTERNS: &[&str] = &[
        "docker",  // docker0, docker_gwbridge
        "virbr",   // libvirt
        "vbox",    // VirtualBox host-only / NAT
        "vmnet",   // VMware
        "vmware",  // VMware Fusion
        "tun",     // VPN
        "tap",     // VPN
        "utun",    // macOS user-space tunnel
        "lo",      // loopback (lo, lo0)
        "loopback",
        "veth",    // container veth pairs
        "bridge",  // generic bridge
        "br-",     // docker user bridges br-xxxxxx
        "anpi",    // macOS app provider
        "awdl",    // Apple Wireless Direct Link (虚拟)
        "llw",     // macOS low-latency WLAN (虚拟)
        "ap",      // Apple AP (单独的 ap1 通常是虚拟)
        "stf",     // 6to4 tunnel
        "gif",     // generic tunnel
    ];
    !VIRTUAL_PATTERNS.iter().any(|pat| lower.contains(pat))
}

/// MAC 看起来是物理网卡? (按 OUI 前缀兜底, 名字查不到时用)
///
/// `mac` 是小写 `xx:xx:xx:xx:xx:xx` 格式.
///
/// 规则:
/// - 全零 / locally-administered bit (第一字节第 2 bit = 1) → false (虚拟可能性高)
/// - 已知虚拟 OUI 前缀 (Docker / VMware / VirtualBox / Parallels) → false
/// - 其他 → true
#[must_use]
pub fn is_physical_mac(mac: &str) -> bool {
    if mac.len() < 17 {
        return false;
    }
    // 解析第一字节判断 locally-administered bit (虚拟网卡通常会置位)
    let first_byte_str = &mac[..2];
    let Ok(first_byte) = u8::from_str_radix(first_byte_str, 16) else {
        return false;
    };
    if first_byte & 0x02 != 0 {
        // locally administered — 虚拟网卡常用
        return false;
    }

    // 已知虚拟 OUI 前缀 (前 8 字符, 含两个冒号)
    const VIRTUAL_OUI_PREFIXES: &[&str] = &[
        "00:05:69", // VMware ESX
        "00:0c:29", // VMware Workstation
        "00:1c:14", // VMware
        "00:50:56", // VMware
        "00:1c:42", // Parallels
        "08:00:27", // VirtualBox
        "0a:00:27", // VirtualBox host-only
        "00:15:5d", // Hyper-V
        "00:03:ff", // Microsoft Virtual Server
        "02:42",    // Docker (任何 02:42:xx:xx:xx:xx, 前 5 字符)
    ];
    for prefix in VIRTUAL_OUI_PREFIXES {
        if mac.starts_with(prefix) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同进程内连跑两次 `collect_fingerprint` 必须得到完全相同结果.
    /// 这是 v0.13.0 服务端 `sha256` 稳定性的底线.
    #[test]
    fn fingerprint_is_consistent() {
        let a = collect_fingerprint();
        let b = collect_fingerprint();
        assert_eq!(a, b, "两次采集结果必须一致 (否则 server hash 永远变, 防 spam 失效)");
    }

    /// 至少 hostname / os / cpu / mem 里有 1 个非默认 — 完全 unknown 一定是采集挂了.
    #[test]
    fn fingerprint_has_required_fields() {
        let fp = collect_fingerprint();
        let all_default = fp.hostname == "unknown"
            && fp.os == "unknown"
            && fp.cpu_brand == "unknown"
            && fp.total_memory_mb == 0;
        assert!(
            !all_default,
            "全字段默认 = 采集挂了 ({fp:?}) — CI 环境上 sysinfo 也应该至少拿到 hostname"
        );
    }

    /// canonical JSON 必须确定性: 同 input → 同 output 字符串.
    #[test]
    fn canonical_json_is_deterministic() {
        let fp = MachineFingerprint {
            hostname: "test-host".into(),
            mac_addresses: vec!["aa:bb:cc:dd:ee:ff".into(), "11:22:33:44:55:66".into()],
            os: "macos".into(),
            os_version: "14.5.0".into(),
            cpu_brand: "Apple M2 Pro".into(),
            cpu_cores: 12,
            total_memory_mb: 32_768,
        };
        let a = fingerprint_to_canonical_json(&fp);
        let b = fingerprint_to_canonical_json(&fp);
        let c = fingerprint_to_canonical_json(&fp);
        assert_eq!(a, b);
        assert_eq!(b, c);
        // 字段顺序: hostname → mac → os → os_version → cpu_brand → cpu_cores → total_memory_mb
        let h_pos = a.find("hostname").expect("missing hostname");
        let m_pos = a.find("mac_addresses").expect("missing mac_addresses");
        let o_pos = a.find("\"os\"").expect("missing os");
        let cb_pos = a.find("cpu_brand").expect("missing cpu_brand");
        assert!(h_pos < m_pos, "hostname 必须先于 mac_addresses");
        assert!(m_pos < o_pos, "mac_addresses 必须先于 os");
        assert!(o_pos < cb_pos, "os 必须先于 cpu_brand");
    }

    /// canonical JSON 出来后能 from_str 还原 — 反向证明 serde 闭环.
    #[test]
    fn canonical_json_parseable() {
        let fp = MachineFingerprint {
            hostname: "frank-dev".into(),
            mac_addresses: vec!["00:11:22:33:44:55".into()],
            os: "linux".into(),
            os_version: "6.1.0".into(),
            cpu_brand: "AMD Ryzen 9 7950X".into(),
            cpu_cores: 32,
            total_memory_mb: 65_536,
        };
        let json = fingerprint_to_canonical_json(&fp);
        let back: MachineFingerprint =
            serde_json::from_str(&json).expect("canonical JSON 必须能反向解析");
        assert_eq!(fp, back);
    }

    /// `is_physical_mac` 单独测过滤逻辑 — 不依赖系统真实网卡.
    #[test]
    fn virtual_macs_filtered() {
        // 物理网卡 (Apple OUI / 真实厂商) — 应该留
        assert!(is_physical_mac("3c:22:fb:aa:bb:cc"), "Apple OUI 是物理");
        assert!(is_physical_mac("dc:a6:32:11:22:33"), "Raspberry Pi OUI 是物理");
        assert!(is_physical_mac("a4:83:e7:de:ad:be"), "Apple OUI 是物理");

        // 虚拟网卡 — 应该过滤
        assert!(!is_physical_mac("02:42:ac:11:00:02"), "Docker 02:42 应过滤");
        assert!(!is_physical_mac("08:00:27:11:22:33"), "VirtualBox 08:00:27 应过滤");
        assert!(!is_physical_mac("0a:00:27:00:00:00"), "VirtualBox host-only 应过滤");
        assert!(!is_physical_mac("00:50:56:c0:00:01"), "VMware 00:50:56 应过滤");
        assert!(!is_physical_mac("00:0c:29:aa:bb:cc"), "VMware Workstation 应过滤");
        assert!(!is_physical_mac("00:15:5d:01:02:03"), "Hyper-V 应过滤");

        // locally-administered bit 置位 — 应过滤
        assert!(
            !is_physical_mac("06:aa:bb:cc:dd:ee"),
            "locally-administered bit (0x02) 应过滤"
        );
        assert!(
            !is_physical_mac("0a:11:22:33:44:55"),
            "locally-administered bit 应过滤 (0a = 00001010)"
        );

        // 畸形输入 — 不 panic, 返回 false
        assert!(!is_physical_mac(""));
        assert!(!is_physical_mac("not-a-mac"));
        assert!(!is_physical_mac("xx:yy:zz:aa:bb:cc"));
    }

    /// 名字过滤逻辑 — 已知虚拟网卡名都识别得出.
    #[test]
    fn virtual_iface_names_filtered() {
        // 物理网卡名 — 留
        assert!(is_physical_iface_name("en0"));
        assert!(is_physical_iface_name("eth0"));
        assert!(is_physical_iface_name("wlan0"));
        assert!(is_physical_iface_name("Ethernet"));

        // 虚拟 — 过滤
        assert!(!is_physical_iface_name("docker0"));
        assert!(!is_physical_iface_name("vboxnet0"));
        assert!(!is_physical_iface_name("vmnet1"));
        assert!(!is_physical_iface_name("utun0"));
        assert!(!is_physical_iface_name("tun0"));
        assert!(!is_physical_iface_name("lo"));
        assert!(!is_physical_iface_name("lo0"));
        assert!(!is_physical_iface_name("br-1234abcd"));
        assert!(!is_physical_iface_name("virbr0"));
    }

    /// 空 MAC 列表 (全过滤掉) 边界场景 — fingerprint 仍可用.
    #[test]
    fn empty_mac_list_handled() {
        let fp = MachineFingerprint {
            hostname: "isolated".into(),
            mac_addresses: Vec::new(),
            os: "linux".into(),
            os_version: "5.15.0".into(),
            cpu_brand: "Intel Xeon".into(),
            cpu_cores: 4,
            total_memory_mb: 8_192,
        };
        // 空 mac list 不影响序列化
        let json = fingerprint_to_canonical_json(&fp);
        assert!(json.contains("\"mac_addresses\":[]"), "空 mac 列表应序列化为 []");
        // 反向解析
        let back: MachineFingerprint = serde_json::from_str(&json).unwrap();
        assert!(back.mac_addresses.is_empty());
        assert_eq!(back, fp);
    }

    /// OS 名规范化 — Darwin → macos / Linux → linux / Windows → windows.
    #[test]
    fn os_name_normalization() {
        assert_eq!(normalize_os_name("Darwin"), "macos");
        assert_eq!(normalize_os_name("darwin"), "macos");
        assert_eq!(normalize_os_name("macOS"), "macos");
        assert_eq!(normalize_os_name("Linux"), "linux");
        assert_eq!(normalize_os_name("Ubuntu"), "linux"); // sysinfo 部分系统返回发行版名
        assert_eq!(normalize_os_name("Windows"), "windows");
        assert_eq!(normalize_os_name("Windows 11"), "windows");
        assert_eq!(normalize_os_name(""), "unknown");
    }

    /// `Default::default()` 出来的 fingerprint 是合法可序列化的 (兜底防御).
    #[test]
    fn default_fingerprint_is_valid() {
        let fp = MachineFingerprint::default();
        let json = fingerprint_to_canonical_json(&fp);
        let back: MachineFingerprint = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fp);
        assert_eq!(back.hostname, "unknown");
        assert_eq!(back.cpu_cores, 0);
    }
}
