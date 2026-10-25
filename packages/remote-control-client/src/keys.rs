//! Long-term Ed25519 identity keys, ephemeral X25519 keys and HKDF, on `ring`.
//!
//! An [`IdentityKey`] can be generated, imported and exported as the 32-byte seed
//! or the PKCS#8 encoding the TypeScript clients store, so a host app may keep
//! it in a Keystore or Keychain. Platforms whose keystore signs without ever
//! exporting the key implement [`Signer`] instead.

use async_trait::async_trait;
use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::hkdf;
use ring::rand::SystemRandom;
use ring::signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey as SigPublicKey};
use zeroize::Zeroizing;

use crate::encoding::{b64u_encode, fill_random, sha256};
use crate::error::ProtocolError;

/// PKCS#8 header of an Ed25519 private key; the 32-byte seed follows.
pub const ED25519_PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// Bytes of an Ed25519 public key and of an X25519 public key.
pub const KEY_BYTES: usize = 32;
/// Bytes of an Ed25519 signature.
pub const SIGNATURE_BYTES: usize = 64;

/// A signing failure reported by a [`Signer`] (a platform keystore, typically).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("signing failed: {0}")]
pub struct SignerError(pub String);

/// Something that holds an Ed25519 private key and can sign with it.
///
/// [`IdentityKey`] is the in-process implementation. A mobile app may implement
/// this over its platform keystore so the key never leaves it.
#[async_trait]
pub trait Signer: Send + Sync {
    /// The 32-byte raw public key.
    fn public_key(&self) -> [u8; KEY_BYTES];
    /// The Ed25519 signature (64 bytes) of `message`.
    async fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_BYTES], SignerError>;
}

/// A long-term Ed25519 signing key (a device key or a host key).
pub struct IdentityKey {
    key_pair: Ed25519KeyPair,
    seed: Zeroizing<[u8; KEY_BYTES]>,
    public: [u8; KEY_BYTES],
}

impl std::fmt::Debug for IdentityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityKey")
            .field("id", &id_from_key(&self.public))
            .finish_non_exhaustive()
    }
}

impl IdentityKey {
    /// Generate a fresh key from the operating system's random source.
    pub fn generate() -> Self {
        let mut seed = Zeroizing::new([0u8; KEY_BYTES]);
        fill_random(seed.as_mut());
        Self::from_seed(seed.as_ref()).expect("every 32-byte seed is a valid Ed25519 key")
    }

    /// The key with this 32-byte seed (RFC 8032 private key).
    pub fn from_seed(seed: &[u8]) -> Result<Self, ProtocolError> {
        let key_pair = Ed25519KeyPair::from_seed_unchecked(seed)
            .map_err(|_| ProtocolError::new("Ed25519 seed must be 32 bytes"))?;
        let mut public = [0u8; KEY_BYTES];
        public.copy_from_slice(key_pair.public_key().as_ref());
        let mut kept = Zeroizing::new([0u8; KEY_BYTES]);
        kept.copy_from_slice(seed);
        Ok(Self {
            key_pair,
            seed: kept,
            public,
        })
    }

    /// Import a PKCS#8 encoding: the 16-byte Ed25519 header followed by the seed
    /// (48 bytes), which is what WebCrypto exports and the other clients store.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, ProtocolError> {
        match pkcs8.strip_prefix(ED25519_PKCS8_PREFIX.as_slice()) {
            Some(seed) if seed.len() == KEY_BYTES => Self::from_seed(seed),
            _ => Err(ProtocolError::new("not an Ed25519 PKCS#8 private key")),
        }
    }

    /// The 32-byte seed, for storing the key. Treat it as a secret.
    pub fn seed(&self) -> Zeroizing<[u8; KEY_BYTES]> {
        self.seed.clone()
    }

    /// The PKCS#8 encoding (header and seed, 48 bytes), for storing the key. Treat it as a secret.
    pub fn to_pkcs8(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(48));
        out.extend_from_slice(&ED25519_PKCS8_PREFIX);
        out.extend_from_slice(self.seed.as_ref());
        out
    }

    /// The 32-byte raw public key.
    pub fn public_key(&self) -> [u8; KEY_BYTES] {
        self.public
    }

    /// The public key as `b64u`, the form records carry.
    pub fn public_key_b64u(&self) -> String {
        b64u_encode(&self.public)
    }

    /// The self-certifying id of the public key.
    pub fn id(&self) -> String {
        id_from_key(&self.public)
    }

    /// The Ed25519 signature of `message`.
    pub fn sign_bytes(&self, message: &[u8]) -> [u8; SIGNATURE_BYTES] {
        let mut out = [0u8; SIGNATURE_BYTES];
        out.copy_from_slice(self.key_pair.sign(message).as_ref());
        out
    }
}

#[async_trait]
impl Signer for IdentityKey {
    fn public_key(&self) -> [u8; KEY_BYTES] {
        self.public
    }

    async fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_BYTES], SignerError> {
        Ok(self.sign_bytes(message))
    }
}

/// Check an Ed25519 signature. Never fails loudly: a wrong-sized key or
/// signature and an invalid point all answer `false`.
pub fn verify_signature(public_key: &[u8], signature_bytes: &[u8], message: &[u8]) -> bool {
    if public_key.len() != KEY_BYTES || signature_bytes.len() != SIGNATURE_BYTES {
        return false;
    }
    SigPublicKey::new(&signature::ED25519, public_key)
        .verify(message, signature_bytes)
        .is_ok()
}

/// Self-certifying id of a public key: the first 22 characters of `b64u(SHA-256(raw key))`.
pub fn id_from_key(public_key: &[u8]) -> String {
    let mut id = b64u_encode(&sha256(public_key));
    id.truncate(22);
    id
}

/// A single-connection X25519 key. Consumed by [`EphemeralKey::agree`], so it
/// cannot be reused for a second connection.
pub struct EphemeralKey {
    private: EphemeralPrivateKey,
    public: [u8; KEY_BYTES],
}

impl std::fmt::Debug for EphemeralKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EphemeralKey").finish_non_exhaustive()
    }
}

impl EphemeralKey {
    /// Generate a fresh key.
    ///
    /// # Panics
    /// Panics if the operating system's random source fails.
    pub fn generate() -> Self {
        let rng = SystemRandom::new();
        let private = EphemeralPrivateKey::generate(&X25519, &rng)
            .expect("the operating system random source failed");
        let mut public = [0u8; KEY_BYTES];
        public.copy_from_slice(
            private
                .compute_public_key()
                .expect("an X25519 public key can always be computed")
                .as_ref(),
        );
        Self { private, public }
    }

    /// The 32-byte raw public key.
    pub fn public_key(&self) -> [u8; KEY_BYTES] {
        self.public
    }

    /// X25519 shared secret with the peer's public key. A peer key that yields the
    /// all-zero secret (a low-order point) is rejected.
    pub fn agree(self, peer_public: &[u8]) -> Result<SharedSecret, ProtocolError> {
        if peer_public.len() != KEY_BYTES {
            return Err(ProtocolError::new("peer public key must be 32 bytes"));
        }
        let peer = UnparsedPublicKey::new(&X25519, peer_public);
        let secret = agreement::agree_ephemeral(self.private, &peer, |shared| {
            let mut out = [0u8; KEY_BYTES];
            out.copy_from_slice(shared);
            out
        })
        .map_err(|_| ProtocolError::new("X25519 key agreement failed"))?;
        let secret = SharedSecret(Zeroizing::new(secret));
        if secret.0.iter().all(|byte| *byte == 0) {
            return Err(ProtocolError::new("X25519 shared secret is all zero"));
        }
        Ok(secret)
    }
}

/// The 32-byte output of an X25519 agreement. Wiped on drop; never printed.
pub struct SharedSecret(Zeroizing<[u8; KEY_BYTES]>);

impl SharedSecret {
    /// Wrap raw secret bytes (fixtures, tests).
    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The raw secret bytes.
    pub fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

impl std::fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedSecret(..)")
    }
}

struct OutputLen(usize);

impl hkdf::KeyType for OutputLen {
    fn len(&self) -> usize {
        self.0
    }
}

/// HKDF-SHA256 (RFC 5869) producing `length` bytes.
pub fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8], length: usize) -> Zeroizing<Vec<u8>> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    let info = [info];
    let okm = prk
        .expand(&info, OutputLen(length))
        .expect("HKDF-SHA256 output length is at most 255 * 32 bytes");
    let mut out = Zeroizing::new(vec![0u8; length]);
    okm.fill(out.as_mut_slice())
        .expect("the output buffer has the requested length");
    out
}
