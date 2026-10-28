//! Signs release files so the in-app updater trusts them (an extra check on top of GitHub's).
//!
//!   cargo run --release --example sign_release -- keygen <key file>
//!       makes a new key pair: the private key goes to <key file> (keep it safe and private,
//!       outside the repo), the public key is printed: it goes in UPDATE_KEY in src/updater.rs
//!
//!   cargo run --release --example sign_release -- sign <key file> <files…>
//!       writes signatures.txt (every file's signature) next to the files, and <file>.sig for each
//!       one; upload them to the release with the files (docs/RELEASING.md says until when the
//!       .sig files are needed)
//!
//! What's signed: "<file name>\n<sha-256 of the file>", so a signature can't be moved to
//! another file (or an older version, whose file name has another version number).

use std::fs;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") if args.len() == 2 => {
            let path = Path::new(&args[1]);
            if path.exists() {
                eprintln!("{} already exists: not overwriting a key", path.display());
                std::process::exit(1);
            }
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed).expect("no randomness");
            let key = SigningKey::from_bytes(&seed);
            fs::write(path, hex(&seed)).expect("can't write the key");
            println!("private key: {}", path.display());
            println!("public key (UPDATE_KEY): {}", hex(key.verifying_key().as_bytes()));
        }
        Some("sign") if args.len() >= 3 => {
            let seed = unhex(fs::read_to_string(&args[1]).expect("can't read the key").trim());
            let key = SigningKey::from_bytes(&seed.try_into().expect("bad key file"));
            let mut list = String::new();
            let mut dir = None;
            for f in &args[2..] {
                let p = Path::new(f);
                let name = p.file_name().and_then(|n| n.to_str()).expect("bad file name");
                // Signatures themselves aren't signed (a second run over release/* skips them)
                if name.ends_with(".sig") || name == "signatures.txt" {
                    continue;
                }
                let digest = hex(&Sha256::digest(fs::read(p).expect("can't read the file")));
                let sig = hex(&key.sign(format!("{name}\n{digest}").as_bytes()).to_bytes());
                fs::write(format!("{f}.sig"), &sig).expect("can't write the signature");
                list.push_str(&format!("{sig}  {name}\n"));
                dir.get_or_insert_with(|| p.parent().map(Path::to_path_buf).unwrap_or_default());
                println!("signed {name}");
            }
            if let Some(dir) = dir {
                fs::write(dir.join("signatures.txt"), list).expect("can't write signatures.txt");
                println!("wrote {}", dir.join("signatures.txt").display());
            }
        }
        _ => {
            eprintln!("usage: sign_release keygen <key file> | sign <key file> <files…>");
            std::process::exit(2);
        }
    }
}
