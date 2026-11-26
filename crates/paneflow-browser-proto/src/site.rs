use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    Secure,
    MixedContent,
    CertificateError,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificateName {
    pub common_name: String,
    pub organization: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Certificate {
    pub subject: CertificateName,
    pub issuer: CertificateName,
    pub valid_from: i64,
    pub valid_until: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteSecurity {
    pub url: String,
    pub connection: Connection,
    pub protocol: Option<String>,
    pub certificate: Option<Certificate>,
}
