//! 固定评测子集清单：把实例选择从运行时随机行为变成版本化输入。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const EVAL_SUITE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixedEvalSuite {
    pub schema_version: u32,
    pub suite_id: String,
    pub dataset: String,
    pub dataset_revision: String,
    pub split: String,
    pub expected_instances: usize,
    pub instances: Vec<String>,
}

pub fn parse_fixed_suite(json: &str) -> Result<FixedEvalSuite, String> {
    let suite: FixedEvalSuite = serde_json::from_str(json)
        .map_err(|e| format!("suite manifest JSON 无法解析：{e}"))?;
    validate_fixed_suite(&suite)?;
    Ok(suite)
}

pub fn validate_fixed_suite(suite: &FixedEvalSuite) -> Result<(), String> {
    if suite.schema_version != EVAL_SUITE_SCHEMA_VERSION {
        return Err(format!("不支持 suite schema {}", suite.schema_version));
    }
    if suite.suite_id.is_empty()
        || !suite.suite_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err("suite_id 只能包含字母、数字、点、短横线和下划线".into());
    }
    if suite.dataset.trim().is_empty() || suite.dataset_revision.trim().is_empty() || suite.split.trim().is_empty() {
        return Err("dataset/dataset_revision/split 不能为空".into());
    }
    if suite.dataset_revision.len() != 40
        || !suite.dataset_revision.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err("dataset_revision 必须是 40 位十六进制提交哈希".into());
    }
    if suite.expected_instances == 0 || suite.instances.len() != suite.expected_instances {
        return Err(format!(
            "suite 实例数不一致：expected={} actual={}",
            suite.expected_instances,
            suite.instances.len()
        ));
    }
    let mut seen = HashSet::with_capacity(suite.instances.len());
    for id in &suite.instances {
        let valid = id.len() <= 160
            && id.contains("__")
            && id.rsplit_once('-').is_some_and(|(_, n)| n.chars().all(|c| c.is_ascii_digit()));
        if !valid { return Err(format!("无效 SWE-bench instance_id：{id}")); }
        if !seen.insert(id) { return Err(format!("suite 包含重复 instance_id：{id}")); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite(count: usize) -> FixedEvalSuite {
        FixedEvalSuite {
            schema_version: 1,
            suite_id: "swe-bench-verified-smoke-25-v1".into(),
            dataset: "princeton-nlp/SWE-bench_Verified".into(),
            dataset_revision: "c104f840cc67f8b6eec6f759ebc8b2693d585d4a".into(),
            split: "test".into(),
            expected_instances: count,
            instances: (0..count).map(|n| format!("owner__repo-{}", n + 1)).collect(),
        }
    }

    #[test]
    fn accepts_exact_unique_manifest() { validate_fixed_suite(&suite(25)).unwrap(); }

    #[test]
    fn rejects_count_drift_and_duplicates() {
        let mut value = suite(25);
        value.expected_instances = 24;
        assert!(validate_fixed_suite(&value).is_err());
        let mut value = suite(25);
        value.instances[1] = value.instances[0].clone();
        assert!(validate_fixed_suite(&value).is_err());
    }

    #[test]
    fn repository_verified_25_manifest_is_valid() {
        let json = include_str!("../../../evals/suites/swe-bench-verified-smoke-25-v1.json");
        let suite = parse_fixed_suite(json).unwrap();
        assert_eq!(suite.instances.len(), 25);
        assert_eq!(suite.dataset_revision.len(), 40);
    }
}
