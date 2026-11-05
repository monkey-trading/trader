use std::path::PathBuf;

use engine_core::eip712::recover;

use super::*;

/// The Web3 Secret Storage definition's PBKDF2 test vector: password `testpassword`.
const PBKDF2_KEYSTORE: &str = r#"{
  "crypto": {
    "cipher": "aes-128-ctr",
    "cipherparams": { "iv": "6087dab2f9fdbbfaddc31a909735c1e6" },
    "ciphertext": "5318b4d5bcd28de64ee5559e671353e16f075ecae9f99c7a79a38af5f869aa46",
    "kdf": "pbkdf2",
    "kdfparams": {
      "c": 262144,
      "dklen": 32,
      "prf": "hmac-sha256",
      "salt": "ae3cd4e7013836a3df6bd7241b12db061dbe2c6785853cce422d148a624ce0bd"
    },
    "mac": "517ead924a9d0dc3124507e3393d175ce3ff7c1e96529c6c555ce9e51205e9b2"
  },
  "id": "3198bc9c-6672-5ab3-d995-4942343ae5b6",
  "version": 3
}"#;

/// The same key under the same password in the scrypt keystore wallets write (n 8192, r 8, p 1), as
/// Foundry's `cast wallet import` wrote it.
const SCRYPT_KEYSTORE: &str = r#"{
  "crypto": {
    "cipher": "aes-128-ctr",
    "cipherparams": { "iv": "30ea5d2ac7e69120a11e9bb3827c8aaa" },
    "ciphertext": "62c9c9b28235caf7fde5d5b73b7e2a873e66f05dbb5fa9f2019ef9b4d316b4e5",
    "kdf": "scrypt",
    "kdfparams": {
      "dklen": 32,
      "n": 8192,
      "p": 1,
      "r": 8,
      "salt": "e55c65cd4e1f3324bba401e22dd2c05a06f7e79e6c48a9fec1545aa31104a12c"
    },
    "mac": "a14acc7645c83cd48ff90a3be845f6db466eaa7132b748c043785db052008543"
  },
  "id": "77544f4c-fd83-42b4-a567-21cc0d30330b",
  "version": 3
}"#;

/// The address of the vectors' key, 7a28b5ba…7514fe9d.
const VECTOR_ADDRESS: &str = "0x008aeeda4d805471df9b2a5b0f38a0c3bcba786b";

/// Both keystores unlock to the key they encrypt, with the password from a file or a variable.
#[tokio::test]
async fn keystores_unlock_with_their_password() {
    let dir = tempfile::tempdir().unwrap();
    let password_file = write(&dir, "password", "testpassword\n");
    for (name, keystore) in [("pbkdf2.json", PBKDF2_KEYSTORE), ("scrypt.json", SCRYPT_KEYSTORE)] {
        let path = write(&dir, name, keystore);
        let from_file =
            KeySource::Keystore { path: path.clone(), password: PasswordSource::File(password_file.clone()) };
        let key = AccountKey::load(&from_file, no_env).await.unwrap();
        assert_eq!(key.address(), VECTOR_ADDRESS.parse().unwrap(), "{name}");
        let from_env = KeySource::Keystore { path, password: PasswordSource::Env("PASSWORD".into()) };
        let env = |name: &str| (name == "PASSWORD").then(|| "testpassword".to_owned());
        assert_eq!(AccountKey::load(&from_env, env).await.unwrap().address(), key.address(), "{name}");
    }
}

/// A wrong password, an unset variable, or a missing file unlocks nothing.
#[tokio::test]
async fn a_keystore_refuses_anything_but_its_password() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "pbkdf2.json", PBKDF2_KEYSTORE);
    let wrong =
        KeySource::Keystore { path: path.clone(), password: PasswordSource::File(write(&dir, "p", "testpasswor")) };
    let err = AccountKey::load(&wrong, no_env).await.unwrap_err();
    assert!(format!("{err:#}").contains("decrypting"), "{err:#}");
    let unset = KeySource::Keystore { path, password: PasswordSource::Env("UNSET".into()) };
    let err = AccountKey::load(&unset, no_env).await.unwrap_err();
    assert!(err.to_string().contains("UNSET is not set"), "{err}");
    let missing =
        KeySource::Keystore { path: dir.path().join("missing.json"), password: PasswordSource::Env("PASSWORD".into()) };
    assert!(AccountKey::load(&missing, |_| Some("testpassword".into())).await.is_err());
}

/// One trailing newline (or CRLF) is not part of a password file's password; anything else is.
#[test]
fn password_files_drop_one_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    for (text, password) in [("pw\n", "pw"), ("pw\r\n", "pw"), ("pw", "pw"), ("pw\n\n", "pw\n"), (" pw ", " pw ")] {
        let read = read_password(&PasswordSource::File(write(&dir, "p", text)), no_env).unwrap();
        assert_eq!(read.as_str(), password, "{text:?}");
    }
}

/// A hex key file holds the key, with or without `0x` and surrounding whitespace; the key whose
/// secret is 1 is the well-known address 0x7e5f…bdf, and it signs as that address.
#[tokio::test]
async fn hex_key_files_hold_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let one = format!("{:064x}", 1);
    for text in [one.clone(), format!("0x{one}\n"), format!("  {one}  ")] {
        let key = AccountKey::load(&KeySource::HexFile { path: write(&dir, "key", &text) }, no_env).await.unwrap();
        let expected = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf".parse().unwrap();
        assert_eq!(key.address(), expected, "{text:?}");
        let hash = B256::repeat_byte(7);
        assert_eq!(recover(&hash, &key.sign(&hash)), Some(expected));
    }
    for bad in [&one[..62], "zz", "", &format!("{one}00")] {
        let err = AccountKey::load(&KeySource::HexFile { path: write(&dir, "key", bad) }, no_env).await.unwrap_err();
        assert!(err.to_string().contains("64 hex characters"), "{bad:?}: {err}");
    }
    let zero = format!("{:064x}", 0);
    assert!(AccountKey::load(&KeySource::HexFile { path: write(&dir, "key", &zero) }, no_env).await.is_err());
}

/// An environment password is taken as it is, trailing newline included.
#[test]
fn environment_passwords_are_verbatim() {
    for value in ["pw\n", "pw\r\n", " pw "] {
        let env = |_: &str| Some(value.to_owned());
        assert_eq!(read_password(&PasswordSource::Env("P".into()), env).unwrap().as_str(), value, "{value:?}");
    }
}

/// Every key whose secret the repository publishes is refused outside a dev build, and taken in
/// one; any other key is taken in both.
#[test]
fn published_keys_are_dev_only() {
    let mut secrets: Vec<[u8; 32]> = [0x11, 0x51, 0x52, 0x42].map(|byte| [byte; 32]).into();
    for hex in [
        "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
        "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
        "7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
        "47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
        "8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
        "92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e",
        "4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356",
        "7a28b5ba57c53603b0b07b56bba752f7784bf506fa95edc395f5cf6c7514fe9d",
        &format!("{:064x}", 1),
        &format!("{:064x}", 0x1234_5678_9abc_u64),
    ] {
        let mut secret = [0u8; 32];
        hex::decode_to_slice(hex, &mut secret).unwrap();
        secrets.push(secret);
    }
    assert_eq!(secrets.len(), PUBLISHED_ACCOUNTS.len());
    for secret in secrets {
        let account = AccountKey::from_secret(&secret).unwrap().address();
        let err = refuse_published(account, false).unwrap_err();
        assert!(err.to_string().contains("published for development"), "{account}: {err}");
        refuse_published(account, true).unwrap();
    }
    let other = AccountKey::from_secret(&[0x77; 32]).unwrap().address();
    refuse_published(other, false).unwrap();
}

/// A key prints as its address only, never its secret.
#[tokio::test]
async fn a_key_prints_only_its_address() {
    let dir = tempfile::tempdir().unwrap();
    let secret = format!("{:064x}", 0x1234_5678_9abc_u64);
    let key = AccountKey::load(&KeySource::HexFile { path: write(&dir, "key", &secret) }, no_env).await.unwrap();
    let printed = format!("{key:?}");
    assert_eq!(printed, format!("AccountKey({})", key.address()));
    assert!(!printed.contains("123456789abc"));
}

// ── Helpers ──

/// An environment with nothing set.
fn no_env(_: &str) -> Option<String> {
    None
}

/// Writes `text` to `name` in `dir`, and returns its path.
fn write(dir: &tempfile::TempDir, name: &str, text: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, text).unwrap();
    path
}
