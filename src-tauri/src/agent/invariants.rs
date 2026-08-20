//! 文件操作不变式注册表（对齐 deepseek-harness invariants 子系统）：
//! 环境约束 > Prompt 约束——写文件前必须满足的硬性不变式。
//! 新增不变式 = 往 INVARIANTS 追加一条（name + 检查函数），全部写路径自动生效，
//! 无需改动各调用点（write_file/edit_file/delete/move/copy/multi_edit 统一经 check_write 拦截）。

use std::path::Path;

/// 一条不变式：check 命中时返回拒绝说明（None = 通过）
pub struct Invariant {
    pub name: &'static str,
    pub check: fn(&Path) -> Option<&'static str>,
}

/// 注册表：按声明顺序检查，返回第一条命中
pub static INVARIANTS: &[Invariant] = &[
    Invariant {
        name: "secrets_env",
        check: check_env_files,
    },
    Invariant {
        name: "secrets_certs",
        check: check_cert_files,
    },
    Invariant {
        name: "migrations_applied",
        check: check_applied_migrations,
    },
];

/// 写路径统一入口：任一不变式命中即拒绝，返回 (不变式名, 拒绝说明)
pub fn check_write(path: &Path) -> Option<(&'static str, &'static str)> {
    for inv in INVARIANTS {
        if let Some(reason) = (inv.check)(path) {
            return Some((inv.name, reason));
        }
    }
    None
}

/// 不变式：环境变量文件（.env*）禁止写入——密钥类配置不应由 Agent 修改
fn check_env_files(p: &Path) -> Option<&'static str> {
    let name = p.file_name()?.to_string_lossy().to_lowercase();
    if name.starts_with(".env") {
        return Some("环境变量文件（.env*）禁止写入：密钥类配置不应由 Agent 修改，请手动编辑");
    }
    None
}

/// 不变式：密钥/证书文件禁止写入（含鸿蒙签名材料）
fn check_cert_files(p: &Path) -> Option<&'static str> {
    let name = p.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".key")
        || name.ends_with(".pem")
        || name.ends_with(".pfx")
        || name.ends_with(".p12")
        || name.ends_with(".keystore")
        || name.ends_with(".cer")
        || name.ends_with(".p7b")
        || name.ends_with(".jks")
    {
        return Some("密钥/证书文件禁止写入（*.key/*.pem/*.pfx/*.p12/*.keystore/*.cer/*.p7b/*.jks，含鸿蒙签名材料）");
    }
    None
}

/// 不变式：已执行的数据库迁移 SQL 不可修改（须新建递增编号文件）；新建文件允许。
/// 按父目录组件名判断（避免项目路径本身含 migrations 字样时误伤全部 .sql 文件）
fn check_applied_migrations(p: &Path) -> Option<&'static str> {
    let name = p.file_name()?.to_string_lossy().to_lowercase();
    if p.exists() && name.ends_with(".sql") {
        let parent_name = p
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if parent_name == "migrations" || parent_name == "migration" {
            return Some("已执行的数据库迁移 SQL 不可修改：请新建递增编号的迁移文件（如 014_xxx.sql）");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("invariants-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rejects_env_and_cert_files() {
        let d = tmp();
        let env = d.join(".env");
        let (name, _) = check_write(&env).unwrap();
        assert_eq!(name, "secrets_env");
        let pem = d.join("debug.pem");
        let (name, _) = check_write(&pem).unwrap();
        assert_eq!(name, "secrets_certs");
        // 普通文件放行
        assert!(check_write(&d.join("main.ets")).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_existing_migration_sql_only() {
        let d = tmp();
        let mig = d.join("migrations");
        std::fs::create_dir_all(&mig).unwrap();
        let applied = mig.join("001_initial.sql");
        std::fs::File::create(&applied).unwrap().write_all(b"x").unwrap();
        let (name, _) = check_write(&applied).unwrap();
        assert_eq!(name, "migrations_applied");
        // 新建（不存在）的迁移文件允许
        let fresh = mig.join("052_reminders.sql");
        assert!(check_write(&fresh).is_none());
        // 非 migrations 目录的 sql 放行
        let other = d.join("scripts").join("seed.sql");
        std::fs::create_dir_all(d.join("scripts")).unwrap();
        std::fs::File::create(&other).unwrap().write_all(b"x").unwrap();
        assert!(check_write(&other).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
