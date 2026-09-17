//! 已审批输入的独立只读副本。随机副本路径不参与逻辑请求身份。
use super::{capability_broker::HostCapability, ota_scope::OtaScope};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(crate) struct OtaInputs {
    directory: PathBuf,
    profile: bool,
}

impl OtaInputs {
    pub(crate) fn create(
        capability: &HostCapability,
        workspace: &Path,
        approved: &OtaScope,
    ) -> Result<Self, String> {
        capability.validate()?;
        let HostCapability::PackageOta {
            hap_path,
            output_path,
            profile_path,
        } = capability
        else {
            return Err("输入副本仅适用于 OTA".into());
        };
        let root = workspace
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let roots = [root.to_string_lossy().into_owned()];
        let output = super::tools::resolve_for_write(&roots, output_path)?;
        let parent = output
            .parent()
            .ok_or("OTA 输出缺少父目录")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !parent.starts_with(&root) {
            return Err("OTA 副本目录越出工作区".into());
        }
        let directory = parent.join(format!(".inputs-{}", uuid::Uuid::new_v4()));
        // unix 上收紧目录权限；分别构造以避免非 unix 平台出现「未使用的 mut」告警
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        builder
            .create(&directory)
            .map_err(|error| format!("创建 OTA 独占副本目录失败：{error}"))?;
        let inputs = Self {
            directory,
            profile: profile_path.is_some(),
        };
        let hap = super::tools::resolve_in_roots(&roots, hap_path)?;
        copy_verified(
            &hap,
            &inputs.directory.join("input.hap"),
            approved.hap_digest(),
            2 * 1024 * 1024 * 1024,
        )?;
        match (profile_path, approved.profile_digest()) {
            (Some(path), Some(digest)) => {
                let profile = super::tools::resolve_in_roots(&roots, path)?;
                copy_verified(
                    &profile,
                    &inputs.directory.join("profile.json"),
                    digest,
                    16 * 1024 * 1024,
                )?;
            }
            (None, None) => {}
            _ => return Err("OTA profile 与审批副本契约不一致".into()),
        }
        Ok(inputs)
    }

    /// 只重定向 Broker 已生成的固定输入参数，不改变 jar、输出、超时或请求摘要。
    pub(crate) fn apply_to_args(&self, args: &mut [String]) -> Result<(), String> {
        let hap = args
            .iter()
            .position(|arg| arg == "--hap")
            .ok_or("OTA 固定 argv 缺少 --hap")?;
        let profile = args.iter().position(|arg| arg == "--profile");
        if hap + 1 >= args.len()
            || profile.is_some() != self.profile
            || profile.is_some_and(|index| index + 1 >= args.len())
        {
            return Err("OTA 固定 argv 与副本契约不一致".into());
        }
        args[hap + 1] = self
            .directory
            .join("input.hap")
            .to_string_lossy()
            .into_owned();
        if let Some(index) = profile {
            args[index + 1] = self
                .directory
                .join("profile.json")
                .to_string_lossy()
                .into_owned();
        }
        Ok(())
    }
}

fn copy_verified(
    source: &Path,
    destination: &Path,
    expected: &str,
    limit: u64,
) -> Result<(), String> {
    let metadata = std::fs::metadata(source).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > limit {
        return Err("OTA 副本源不是有界非空普通文件".into());
    }
    let mut source = std::fs::File::open(source).map_err(|error| error.to_string())?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("创建 OTA 输入副本失败：{error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err("OTA 输入复制超过协作式 30 秒预算".into());
        }
        let count = source
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > limit {
            return Err("OTA 输入复制超限".into());
        }
        digest.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
    }
    if total == 0 || format!("{:x}", digest.finalize()) != expected {
        return Err("OTA 输入副本摘要与审批不一致，未派发打包".into());
    }
    output.sync_all().map_err(|error| error.to_string())?;
    let mut permissions = output
        .metadata()
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_readonly(true);
    output
        .set_permissions(permissions)
        .map_err(|error| error.to_string())?;
    Ok(())
}

impl Drop for OtaInputs {
    fn drop(&mut self) {
        if self.directory.canonicalize().ok().as_ref() != Some(&self.directory) {
            return;
        }
        for name in ["input.hap", "profile.json"] {
            let path = self.directory.join(name);
            // Windows 不允许直接删除 readonly 文件；只修改本目录中的普通副本。
            // 该 clippy lint 的理由是「Unix 上 set_readonly(false) 会让文件 world-writable」；
            // 此分支仅在 Windows 编译，调用目的正是清 FILE_ATTRIBUTE_READONLY 以便删除只读副本。
            #[cfg(windows)]
            #[allow(clippy::permissions_set_readonly_false)]
            if let Ok(metadata) = std::fs::symlink_metadata(&path) {
                if metadata.file_type().is_file() {
                    let mut permissions = metadata.permissions();
                    permissions.set_readonly(false);
                    let _ = std::fs::set_permissions(&path, permissions);
                }
            }
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("ota-inputs-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            let path = path.canonicalize().unwrap();
            std::fs::create_dir(path.join("stage")).unwrap();
            std::fs::write(path.join("app.hap"), b"approved hap").unwrap();
            std::fs::write(path.join("profile.json"), b"approved profile").unwrap();
            Self(path)
        }
        fn capability(&self, profile: bool) -> HostCapability {
            HostCapability::PackageOta {
                hap_path: "app.hap".into(),
                output_path: "stage/artifact.pkg".into(),
                profile_path: profile.then(|| "profile.json".into()),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn snapshots_are_independent_readonly_and_cleaned_after_use() {
        let fixture = Fixture::new();
        let capability = fixture.capability(true);
        let scope = super::super::ota_scope::capability_scope(&capability, &fixture.0).unwrap();
        let inputs = OtaInputs::create(&capability, &fixture.0, &scope).unwrap();
        let directory = inputs.directory.clone();
        std::fs::write(fixture.0.join("app.hap"), b"changed original").unwrap();
        std::fs::write(fixture.0.join("profile.json"), b"changed profile").unwrap();
        assert_eq!(
            std::fs::read(directory.join("input.hap")).unwrap(),
            b"approved hap"
        );
        assert_eq!(
            std::fs::read(directory.join("profile.json")).unwrap(),
            b"approved profile"
        );
        assert!(std::fs::metadata(directory.join("input.hap"))
            .unwrap()
            .permissions()
            .readonly());
        let mut args = [
            "--hap",
            "original",
            "--out",
            "output",
            "--profile",
            "original-profile",
        ]
        .map(String::from);
        inputs.apply_to_args(&mut args).unwrap();
        assert_eq!(Path::new(&args[1]), directory.join("input.hap"));
        assert_eq!(Path::new(&args[5]), directory.join("profile.json"));
        assert_eq!(args[3], "output");
        drop(inputs);
        assert!(!directory.exists());
        assert_eq!(
            std::fs::read(fixture.0.join("app.hap")).unwrap(),
            b"changed original"
        );
    }

    #[test]
    fn changed_source_or_profile_fails_and_cleans_partial_copies() {
        let fixture = Fixture::new();
        let capability = fixture.capability(true);
        let scope = super::super::ota_scope::capability_scope(&capability, &fixture.0).unwrap();
        std::fs::write(fixture.0.join("profile.json"), b"modified").unwrap();
        assert!(OtaInputs::create(&capability, &fixture.0, &scope).is_err());
        assert_eq!(
            std::fs::read_dir(fixture.0.join("stage")).unwrap().count(),
            0
        );
        std::fs::write(fixture.0.join("app.hap"), b"modified").unwrap();
        assert!(OtaInputs::create(&capability, &fixture.0, &scope).is_err());
        assert_eq!(
            std::fs::read_dir(fixture.0.join("stage")).unwrap().count(),
            0
        );
    }

    #[test]
    fn no_profile_and_bad_argv_are_handled_without_partial_rewrite() {
        let fixture = Fixture::new();
        let capability = fixture.capability(false);
        let scope = super::super::ota_scope::capability_scope(&capability, &fixture.0).unwrap();
        let inputs = OtaInputs::create(&capability, &fixture.0, &scope).unwrap();
        let mut args = ["--hap", "original", "--profile", "bad"].map(String::from);
        let before = args.clone();
        assert!(inputs.apply_to_args(&mut args).is_err());
        assert_eq!(args, before);
        let mut args = ["--hap", "original"].map(String::from);
        inputs.apply_to_args(&mut args).unwrap();
        assert!(!inputs.directory.join("profile.json").exists());
    }

    #[test]
    fn copy_rejects_existing_destination_and_limits() {
        let fixture = Fixture::new();
        let source = fixture.0.join("app.hap");
        let destination = fixture.0.join("existing.hap");
        std::fs::write(&destination, b"keep").unwrap();
        let digest = format!("{:x}", Sha256::digest(b"approved hap"));
        assert!(copy_verified(&source, &destination, &digest, 1024).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"keep");
        assert!(copy_verified(&source, &fixture.0.join("new.hap"), &digest, 1).is_err());
        assert!(!fixture.0.join("new.hap").exists());
    }

    #[test]
    fn cleanup_preserves_unknown_files_and_independent_snapshots() {
        let fixture = Fixture::new();
        let capability = fixture.capability(false);
        let scope = super::super::ota_scope::capability_scope(&capability, &fixture.0).unwrap();
        let first = OtaInputs::create(&capability, &fixture.0, &scope).unwrap();
        let second = OtaInputs::create(&capability, &fixture.0, &scope).unwrap();
        assert_ne!(first.directory, second.directory);
        let first_directory = first.directory.clone();
        std::fs::write(first_directory.join("external-note.txt"), b"preserve").unwrap();
        drop(first);
        assert!(!first_directory.join("input.hap").exists());
        assert_eq!(
            std::fs::read(first_directory.join("external-note.txt")).unwrap(),
            b"preserve"
        );
        assert_eq!(
            std::fs::read(second.directory.join("input.hap")).unwrap(),
            b"approved hap"
        );
        let second_directory = second.directory.clone();
        drop(second);
        assert!(!second_directory.exists());
    }
}
