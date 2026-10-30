//! Desktop host compatibility for one CPA credential worker. No native attestation.
//!
//! The host version is verified from official platform release metadata; the engine version remains the
//! actual Cargo build version. Account/system profiles never depend on a filename.
use base64::Engine;
use http::HeaderValue;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::io;
use std::sync::OnceLock;

pub const ORIGINATOR: &str = "Codex Desktop";
pub const APP_VERSION: &str =
    include_str!("../../../deploy/cpa-runtime/desktop-version").trim_ascii();
static WORKER: OnceLock<DesktopIdentity> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopSystem {
    Windows,
    Mac,
}

#[derive(Clone, Debug)]
pub struct DesktopIdentity {
    system: DesktopSystem,
    seed: [u8; 32],
    timezone: TimeZone,
    timezone_name: String,
}

impl DesktopIdentity {
    /// Only CPA-owned metadata is used. Missing/legacy system values mean Mac.
    pub fn from_credential(value: &Value) -> Self {
        let system = if value["codex_client_system"]
            .as_str()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("windows"))
        {
            DesktopSystem::Windows
        } else {
            DesktopSystem::Mac
        };
        // Do not seed from refresh/access tokens, credential_id, or its path.
        // Older files without either identity use the same explicit legacy bucket.
        let account = value["account_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| value["email"].as_str().filter(|s| !s.is_empty()))
            .unwrap_or("legacy-unidentified-account");
        let seed = Sha256::digest(format!(
            "codex-server:desktop-device:v1:{system:?}:{account}"
        ));
        let timezone = std::env::var("TZ")
            .ok()
            .and_then(|name| TimeZone::get(name.trim_start_matches(':')).ok())
            .or_else(|| TimeZone::try_system().ok())
            .filter(|zone| zone.iana_name().is_some())
            .unwrap_or(TimeZone::UTC);
        let timezone_name = match timezone.iana_name() {
            Some("UTC") | None => "Etc/UTC",
            Some(name) => name,
        }
        .to_owned();
        Self {
            system,
            seed: seed.into(),
            timezone,
            timezone_name,
        }
    }

    pub fn user_agent(&self) -> String {
        let platform = match self.system {
            DesktopSystem::Windows => "Windows 10.0.19044; x86_64",
            DesktopSystem::Mac => "Mac OS 15.3.1; x86_64",
        };
        // Same official app-server layout, with explicit compatibility OS/arch
        // parameters and the headless terminal observed in the official artifact.
        format!(
            "{ORIGINATOR}/{} ({platform}) dumb ({ORIGINATOR}; {APP_VERSION})",
            env!("CARGO_PKG_VERSION")
        )
    }

    pub fn attestation(&self) -> HeaderValue {
        self.attestation_at(Timestamp::now())
    }

    fn attestation_at(&self, now: Timestamp) -> HeaderValue {
        // Port of codex2api PR827's account-stable display and daily-session
        // strategy. These are compatibility signals, not measurements of hardware.
        let digest = Sha256::digest([b"desktop-display:v1:".as_slice(), &self.seed].concat());
        let slot = u64::from_be_bytes(digest[..8].try_into().expect("SHA256 prefix")) % 10;
        let displays = [
            (1920.0, 1080.0, 1.0),
            (1920.0, 1080.0, 1.25),
            (1920.0, 1200.0, 1.25),
            (2560.0, 1440.0, 1.25),
            (2560.0, 1440.0, 1.5),
            (2560.0, 1600.0, 1.5),
            (2880.0, 1800.0, 2.0),
            (3840.0, 2160.0, 1.5),
            (3840.0, 2160.0, 2.0),
        ];
        let (width, height, scale): (f64, f64, f64) = displays[slot.saturating_sub(1) as usize];
        let size = ((width / scale).round() + (height / scale).round()).round() as u64;
        let date = now.to_zoned(self.timezone.clone()).date().to_string();
        let mut session: [u8; 32] = Sha256::digest(
            [
                b"desktop-session:v1:".as_slice(),
                &self.seed,
                date.as_bytes(),
            ]
            .concat(),
        )
        .into();
        session[6] = (session[6] & 0x0f) | 0x40;
        session[8] = (session[8] & 0x3f) | 0x80;
        let hex = session[..16]
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>();
        let session = format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        );
        let locale = locale(&self.timezone_name);
        let mut signals = Vec::new();
        head(&mut signals, 5, 7);
        head(&mut signals, 0, 0);
        head(&mut signals, 0, 1);
        head(&mut signals, 0, 1);
        head(&mut signals, 4, 1);
        text(&mut signals, locale);
        head(&mut signals, 0, 2);
        text(&mut signals, locale);
        head(&mut signals, 0, 3);
        text(&mut signals, &self.timezone_name);
        head(&mut signals, 0, 4);
        head(&mut signals, 0, size);
        head(&mut signals, 0, 5);
        if scale.fract() == 0.0 {
            head(&mut signals, 0, scale as u64);
        } else {
            signals.push(0xfb);
            signals.extend_from_slice(&scale.to_be_bytes());
        }
        head(&mut signals, 0, 6);
        text(&mut signals, &session);
        let mut inner = Vec::new();
        head(&mut inner, 5, 3);
        text(&mut inner, "error_code");
        head(
            &mut inner,
            0,
            match self.system {
                DesktopSystem::Windows => 1,
                DesktopSystem::Mac => 2,
            },
        );
        text(&mut inner, "bundle_id");
        text(&mut inner, "com.openai.codex");
        text(&mut inner, "f");
        head(&mut inner, 2, signals.len() as u64);
        inner.extend(signals);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(inner);
        #[derive(serde::Serialize)]
        struct Envelope {
            v: u8,
            s: u8,
            t: String,
        }
        let envelope = serde_json::to_string(&Envelope {
            v: 1,
            s: 0,
            t: format!("v1.{token}"),
        })
        .expect("serializable attestation envelope");
        HeaderValue::from_str(&envelope).expect("ASCII attestation envelope")
    }
}

// Same en-US fallback as the reference strategy. These are fixed compatibility
// locales for common service zones, not the downstream caller's OS or language.
fn locale(timezone: &str) -> &'static str {
    match timezone {
        "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Harbin" | "PRC" => "zh-CN",
        "Asia/Taipei" => "zh-TW",
        "Asia/Hong_Kong" => "zh-HK",
        "Asia/Tokyo" | "Japan" => "ja-JP",
        "Asia/Seoul" | "ROK" => "ko-KR",
        "Asia/Singapore" | "Singapore" => "en-SG",
        "Europe/London" | "GB" => "en-GB",
        "Europe/Paris" => "fr-FR",
        "Europe/Berlin" => "de-DE",
        "Australia/Sydney" | "Australia/Melbourne" => "en-AU",
        _ => "en-US",
    }
}

fn text(output: &mut Vec<u8>, value: &str) {
    head(output, 3, value.len() as u64);
    output.extend_from_slice(value.as_bytes());
}

fn head(output: &mut Vec<u8>, major: u8, value: u64) {
    let prefix = major << 5;
    match value {
        0..=23 => output.push(prefix | value as u8),
        24..=255 => output.extend_from_slice(&[prefix | 24, value as u8]),
        256..=65535 => {
            output.push(prefix | 25);
            output.extend_from_slice(&(value as u16).to_be_bytes());
        }
        65536..=4294967295 => {
            output.push(prefix | 26);
            output.extend_from_slice(&(value as u32).to_be_bytes());
        }
        _ => {
            output.push(prefix | 27);
            output.extend_from_slice(&value.to_be_bytes());
        }
    }
}

/// Called before any auth/network client is constructed, only in a managed worker.
pub fn install_for_worker(file: &crate::CpaCredentialFile) -> io::Result<()> {
    let identity = DesktopIdentity::from_credential(&file.read()?);
    let user_agent = HeaderValue::from_str(&identity.user_agent()).expect("ASCII Desktop identity");
    WORKER
        .set(identity)
        .map_err(|_| io::Error::other("CPA Desktop identity already initialized"))?;
    codex_model_provider_info::install_managed_client_identity(
        user_agent,
        HeaderValue::from_static(ORIGINATOR),
        HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    )
    .map_err(io::Error::other)
}

pub fn worker_identity() -> Option<&'static DesktopIdentity> {
    WORKER.get()
}

#[cfg(test)]
#[path = "cpa_desktop_tests.rs"]
mod tests;
