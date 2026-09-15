//! OTA 审批作用域：有界流式文件摘要 + canonical 路径绑定，不保存文件内容或原始路径。
use super::capability_broker::HostCapability;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct OtaScope {
    paths_sha256: String,
    hap_sha256: String,
    profile_sha256: Option<String>,
}

impl OtaScope {
    pub(crate) fn hap_digest(&self) -> &str {
        &self.hap_sha256
    }
    pub(crate) fn profile_digest(&self) -> Option<&str> {
        self.profile_sha256.as_deref()
    }
}

pub(crate) fn staging_directory(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().ok_or("OTA 输出缺少父目录")?;
    let name = destination.file_name().ok_or("OTA 输出缺少文件名")?;
    let mut stage_name = std::ffi::OsString::from(".");
    stage_name.push(name);
    stage_name.push(".ota-stage");
    Ok(parent.join(stage_name))
}

fn file_digest(path: &Path, limit: u64) -> Result<String, String> {
    // 先排除目录/FIFO/设备文件，避免常见非普通文件在 open 阶段阻塞。
    let path_metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if !path_metadata.is_file() || path_metadata.len() == 0 || path_metadata.len() > limit {
        return Err(format!(
            "OTA 审批输入必须是非空普通文件且不超过 {limit} 字节"
        ));
    }
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("OTA 审批无法读取输入：{error}"))?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() || before.len() == 0 || before.len() > limit {
        return Err(format!(
            "OTA 审批输入必须是非空普通文件且不超过 {limit} 字节"
        ));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        if std::time::Instant::now() >= deadline {
            return Err("OTA 审批文件摘要计算超过 30 秒，请重试或减小输入".into());
        }
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > limit {
            return Err("OTA 审批输入在读取期间增长超限".into());
        }
        digest.update(&buffer[..count]);
    }
    let after = file.metadata().map_err(|error| error.to_string())?;
    if total != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err("OTA 输入在摘要计算期间发生变化，需重新审批".into());
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn capability_scope(
    capability: &HostCapability,
    workspace: &Path,
) -> Result<OtaScope, String> {
    capability.validate()?;
    let HostCapability::PackageOta {
        hap_path,
        output_path,
        profile_path,
    } = capability
    else {
        return Err("OTA 审批作用域只接受 release.package_ota".into());
    };
    let root = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let roots = [root.to_string_lossy().into_owned()];
    let hap = super::tools::resolve_in_roots(&roots, hap_path)?;
    let output = super::tools::resolve_for_write(&roots, output_path)?;
    let profile = profile_path
        .as_ref()
        .map(|path| super::tools::resolve_in_roots(&roots, path))
        .transpose()?;
    let material = serde_json::json!([root, hap, output, profile]);
    Ok(OtaScope {
        paths_sha256: format!("{:x}", Sha256::digest(material.to_string().as_bytes())),
        hap_sha256: file_digest(&hap, 2 * 1024 * 1024 * 1024)?,
        profile_sha256: profile
            .as_deref()
            .map(|path| file_digest(path, 16 * 1024 * 1024))
            .transpose()?,
    })
}

pub(crate) fn argument_scope(
    roots: &[String],
    args: &serde_json::Value,
) -> Result<OtaScope, String> {
    let hap =
        super::tools::resolve_in_roots(roots, args["hap_path"].as_str().ok_or("缺少 hap_path")?)?;
    let output =
        super::tools::resolve_for_write(roots, args["out_path"].as_str().ok_or("缺少 out_path")?)?;
    let profile = args["profile_path"]
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| super::tools::resolve_in_roots(roots, path))
        .transpose()?;
    let root = roots
        .iter()
        .filter_map(|root| Path::new(root).canonicalize().ok())
        .find(|root| {
            hap.starts_with(root)
                && output.starts_with(root)
                && profile.as_ref().is_none_or(|path| path.starts_with(root))
        })
        .ok_or("OTA 输入和输出必须位于同一授权工作区")?;
    let relative = |path: &Path| -> Result<String, String> {
        path.strip_prefix(&root)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|_| "OTA 审批路径越出工作区".into())
    };
    let final_output = relative(&output)?;
    HostCapability::PackageOta {
        hap_path: relative(&hap)?,
        output_path: final_output,
        profile_path: profile.as_deref().map(relative).transpose()?,
    }
    .validate()?;
    let capability = HostCapability::PackageOta {
        hap_path: relative(&hap)?,
        output_path: relative(&staging_directory(&output)?.join("artifact.pkg"))?,
        profile_path: profile.as_deref().map(relative).transpose()?,
    };
    capability_scope(&capability, &root)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("ota-scope-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            let path = path.canonicalize().unwrap();
            std::fs::write(path.join("app.hap"), b"approved hap").unwrap();
            std::fs::write(path.join("profile.json"), b"approved profile").unwrap();
            Self(path)
        }
        fn roots(&self) -> Vec<String> {
            vec![self.0.to_string_lossy().into_owned()]
        }
        fn args(&self) -> serde_json::Value {
            serde_json::json!({"hap_path":"app.hap","out_path":"output/release.pkg","profile_path":"profile.json"})
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn approved_arguments_match_exact_staged_capability() {
        let fixture = Fixture::new();
        let approved = argument_scope(&fixture.roots(), &fixture.args()).unwrap();
        std::fs::create_dir_all(fixture.0.join("output/.release.pkg.ota-stage")).unwrap();
        let capability = HostCapability::PackageOta {
            hap_path: "app.hap".into(),
            output_path: "output/.release.pkg.ota-stage/artifact.pkg".into(),
            profile_path: Some("profile.json".into()),
        };
        assert_eq!(approved, capability_scope(&capability, &fixture.0).unwrap());
        let changed = HostCapability::PackageOta {
            hap_path: "app.hap".into(),
            output_path: "other.pkg".into(),
            profile_path: Some("profile.json".into()),
        };
        assert_ne!(approved, capability_scope(&changed, &fixture.0).unwrap());
    }

    #[test]
    fn content_replacement_invalidates_scope_even_at_same_path_and_size() {
        let fixture = Fixture::new();
        let approved = argument_scope(&fixture.roots(), &fixture.args()).unwrap();
        std::fs::write(fixture.0.join("app.hap"), b"modified hap").unwrap();
        assert_ne!(
            approved,
            argument_scope(&fixture.roots(), &fixture.args()).unwrap()
        );
        std::fs::write(fixture.0.join("app.hap"), b"approved hap").unwrap();
        std::fs::write(fixture.0.join("profile.json"), b"modified profile").unwrap();
        assert_ne!(
            approved,
            argument_scope(&fixture.roots(), &fixture.args()).unwrap()
        );
    }

    #[test]
    fn equal_contents_in_different_workspaces_do_not_share_scope() {
        let first = Fixture::new();
        let second = Fixture::new();
        assert_ne!(
            argument_scope(&first.roots(), &first.args()).unwrap(),
            argument_scope(&second.roots(), &second.args()).unwrap()
        );
        let mut args = first.args();
        args["profile_path"] = serde_json::Value::Null;
        assert_ne!(
            argument_scope(&first.roots(), &first.args()).unwrap(),
            argument_scope(&first.roots(), &args).unwrap()
        );
    }

    #[test]
    fn digest_is_bounded_and_scope_rejects_invalid_outputs() {
        let fixture = Fixture::new();
        assert!(file_digest(&fixture.0.join("app.hap"), 2).is_err());
        assert!(file_digest(&fixture.0, 1024).is_err());
        let mut args = fixture.args();
        args["out_path"] = "release.txt".into();
        assert!(argument_scope(&fixture.roots(), &args).is_err());
        std::fs::write(fixture.0.join("app.hap"), b"").unwrap();
        assert!(argument_scope(&fixture.roots(), &fixture.args()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_retarget_and_workspace_escape_are_detected() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        let alias = fixture.0.join("alias.hap");
        std::os::unix::fs::symlink(fixture.0.join("app.hap"), &alias).unwrap();
        let mut args = fixture.args();
        args["hap_path"] = "alias.hap".into();
        let approved = argument_scope(&fixture.roots(), &args).unwrap();
        std::fs::write(fixture.0.join("other.hap"), b"approved hap").unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(fixture.0.join("other.hap"), &alias).unwrap();
        assert_ne!(approved, argument_scope(&fixture.roots(), &args).unwrap());
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(outside.0.join("app.hap"), &alias).unwrap();
        assert!(argument_scope(&fixture.roots(), &args).is_err());
    }
}
