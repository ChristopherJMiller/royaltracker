//! Generate a VAPID keypair for Web Push, base64url-encoded the way the config
//! and browser expect. Run:
//!   nix develop -c cargo run -q -p royaltracker-notify --example gen_vapid
//!
//! Put `vapid_private_key_b64` (secret) + `vapid_public_key_b64` in the config
//! secret; the public key is also served to browsers to subscribe.

use base64::Engine;
use web_push_native::p256::{elliptic_curve::sec1::ToEncodedPoint, SecretKey};

fn main() {
    let sk = SecretKey::random(&mut rand_core::OsRng);
    let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let priv_bytes = sk.to_be_bytes();
    let pub_point = sk.public_key().to_encoded_point(false);
    println!("vapid_private_key_b64 = {}", b64(&priv_bytes));
    println!("vapid_public_key_b64  = {}", b64(pub_point.as_bytes()));
    println!("vapid_subject         = mailto:you@example.com  # edit me");
}
