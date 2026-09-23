//! The watchdog's identity on disk.
//!
//! The file holds the 32 secret bytes as 64 hex characters and a newline; it is created with
//! mode `0600` on Unix. The pubky derived from it is what chains name in their genesis
//! (§11.2) and what every engagement and receipt is checked against, so the file must
//! outlive the process and the container: lose it and every engagement it holds is stranded,
//! and every chain that named it has to seat a new witness (§11.2, "Changing the engaged
//! set").

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use pubky_common::crypto::Keypair;

/// Load the keypair at `path`, or generate one and write it there. The flag is whether the
/// file was created by this call.
pub fn load_or_create(path: &Path) -> Result<(Keypair, bool), String> {
    match std::fs::read_to_string(path) {
        Ok(text) => decode(&text)
            .map(|k| (k, false))
            .map_err(|e| format!("keypair file {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let keypair = Keypair::random();
            write(path, &keypair).map_err(|e| format!("keypair file {}: {e}", path.display()))?;
            Ok((keypair, true))
        }
        Err(e) => Err(format!("keypair file {}: {e}", path.display())),
    }
}

/// Parse the file's text.
fn decode(text: &str) -> Result<Keypair, String> {
    let bytes = hex::decode(text.trim()).map_err(|e| format!("not hex: {e}"))?;
    let secret: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("expected 32 secret bytes, found {}", bytes.len()))?;
    Ok(Keypair::from_secret(&secret))
}

/// Write the secret, creating parent directories, refusing to overwrite.
fn write(path: &Path, keypair: &Keypair) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(hex::encode(keypair.secret()).as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_keypair_is_read_back_as_the_same_pubky() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("keypair");
        let (first, created) = load_or_create(&path).unwrap();
        assert!(created);
        let (again, created) = load_or_create(&path).unwrap();
        assert!(!created);
        assert_eq!(first.public_key().z32(), again.public_key().z32());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.trim().len(), 64, "32 bytes of hex");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_damaged_file_is_refused_rather_than_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keypair");
        std::fs::write(&path, "not a key\n").unwrap();
        assert!(load_or_create(&path).is_err());
        std::fs::write(&path, hex::encode([7u8; 16])).unwrap();
        assert!(load_or_create(&path).is_err(), "too short");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            hex::encode([7u8; 16]),
            "left as found"
        );
    }
}
