use cef::*;
use paneflow_browser_proto::{Certificate, CertificateName, Connection, SiteSecurity};

const WINDOWS_TO_UNIX_EPOCH_SECONDS: i64 = 11_644_473_600;
const DISPLAYED_INSECURE_CONTENT: u32 = 1;
const RAN_INSECURE_CONTENT: u32 = 2;

pub fn of(browser: &Browser) -> SiteSecurity {
    let entry = browser
        .host()
        .and_then(|host| host.visible_navigation_entry());
    let url = entry
        .as_ref()
        .map(|entry| CefString::from(&entry.url()).to_string())
        .or_else(|| {
            browser
                .main_frame()
                .map(|frame| CefString::from(&frame.url()).to_string())
        })
        .unwrap_or_default();
    let status = entry.and_then(|entry| entry.sslstatus());
    let Some(status) = status.filter(|status| status.is_secure_connection() == 1) else {
        return SiteSecurity {
            url,
            connection: Connection::Plain,
            protocol: None,
            certificate: None,
        };
    };
    let connection = classify(
        is_cert_status_error(status.cert_status()) == 1,
        status.content_status().get_raw(),
    );
    SiteSecurity {
        url,
        connection,
        protocol: protocol_name(&status.sslversion()).map(str::to_string),
        certificate: status.x509_certificate().map(|certificate| Certificate {
            subject: name(certificate.subject()),
            issuer: name(certificate.issuer()),
            valid_from: unix_seconds(certificate.valid_start().val),
            valid_until: unix_seconds(certificate.valid_expiry().val),
        }),
    }
}

fn name(principal: Option<X509CertPrincipal>) -> CertificateName {
    let Some(principal) = principal else {
        return CertificateName {
            common_name: String::new(),
            organization: None,
        };
    };
    let mut organizations = CefStringList::new();
    principal.organization_names(Some(&mut organizations));
    CertificateName {
        common_name: CefString::from(&principal.common_name()).to_string(),
        organization: organizations
            .into_iter()
            .find(|organization| !organization.trim().is_empty()),
    }
}

pub fn classify(certificate_error: bool, content_status: u32) -> Connection {
    if certificate_error {
        Connection::CertificateError
    } else if content_status & (DISPLAYED_INSECURE_CONTENT | RAN_INSECURE_CONTENT) != 0 {
        Connection::MixedContent
    } else {
        Connection::Secure
    }
}

fn protocol_name(version: &SslVersion) -> Option<&'static str> {
    [
        (SslVersion::TLS1_3, "TLS 1.3"),
        (SslVersion::TLS1_2, "TLS 1.2"),
        (SslVersion::TLS1_1, "TLS 1.1"),
        (SslVersion::TLS1, "TLS 1.0"),
        (SslVersion::QUIC, "QUIC"),
        (SslVersion::SSL3, "SSL 3.0"),
        (SslVersion::SSL2, "SSL 2.0"),
    ]
    .into_iter()
    .find(|(known, _)| known.get_raw() == version.get_raw())
    .map(|(_, name)| name)
}

pub fn unix_seconds(windows_microseconds: i64) -> i64 {
    windows_microseconds / 1_000_000 - WINDOWS_TO_UNIX_EPOCH_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_certificate_error_outranks_mixed_content_and_clean_content_is_secure() {
        assert_eq!(
            classify(true, RAN_INSECURE_CONTENT),
            Connection::CertificateError
        );
        assert_eq!(
            classify(false, DISPLAYED_INSECURE_CONTENT),
            Connection::MixedContent
        );
        assert_eq!(
            classify(false, RAN_INSECURE_CONTENT),
            Connection::MixedContent
        );
        assert_eq!(classify(false, 0), Connection::Secure);
    }

    #[test]
    fn a_chromium_time_converts_to_unix_seconds() {
        assert_eq!(unix_seconds(WINDOWS_TO_UNIX_EPOCH_SECONDS * 1_000_000), 0);
        assert_eq!(
            unix_seconds((WINDOWS_TO_UNIX_EPOCH_SECONDS + 1_800_000_000) * 1_000_000 + 999_999),
            1_800_000_000
        );
    }

    #[test]
    fn every_tls_version_has_a_readable_name() {
        assert_eq!(protocol_name(&SslVersion::TLS1_3), Some("TLS 1.3"));
        assert_eq!(protocol_name(&SslVersion::QUIC), Some("QUIC"));
        assert_eq!(protocol_name(&SslVersion::UNKNOWN), None);
    }
}
