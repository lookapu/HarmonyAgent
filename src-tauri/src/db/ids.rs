use rusqlite::{types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef}, Result as RusqliteResult};
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> RusqliteResult<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.0.as_str()))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                String::column_result(value).map(Self)
            }
        }
    };
}

macro_rules! int_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub i64);

        impl $name {
            pub fn new(v: i64) -> Self {
                Self(v)
            }

            pub fn get(self) -> i64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<i64> for $name {
            fn from(v: i64) -> Self {
                Self(v)
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> RusqliteResult<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.0))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                i64::column_result(value).map(Self)
            }
        }
    };
}

string_id!(ProjectId);
string_id!(ConversationId);
string_id!(MessageId);
string_id!(ProviderId);
string_id!(ModelId);
string_id!(RequestLogId);
string_id!(McpServerId);
string_id!(SkillId);
string_id!(KnowledgeId);
string_id!(TaskRunId);
string_id!(MemoryId);

int_id!(VersionId);
int_id!(SystemPromptVersion);
