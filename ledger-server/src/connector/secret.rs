//! 连接器密码加密（AES-256-GCM）
//!
//! 密钥：环境变量 LEDGER_SECRET_KEY（64 位十六进制），未设置时使用 数据目录/secret.key（首次自动生成，权限 600）。
//! 密文格式：hex(12 字节随机 nonce || 密文含认证标签)。备份数据库时需要一并备份密钥文件。

use std::path::Path;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, bail};

pub struct Secret {
    cipher: Aes256Gcm,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> anyhow::Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("不是有效的十六进制字符串");
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(Into::into)).collect()
}

impl Secret {
    /// 读取或生成密钥
    pub fn load(data_dir: &Path) -> anyhow::Result<Self> {
        let key_hex = match std::env::var("LEDGER_SECRET_KEY").ok().filter(|v| !v.trim().is_empty()) {
            Some(k) => k,
            None => {
                let path = data_dir.join("secret.key");
                if path.exists() {
                    std::fs::read_to_string(&path).with_context(|| format!("读取密钥文件失败：{}", path.display()))?
                } else {
                    let key = Aes256Gcm::generate_key(OsRng);
                    let hex = to_hex(&key);
                    std::fs::write(&path, &hex).with_context(|| format!("写入密钥文件失败：{}", path.display()))?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
                    }
                    tracing::info!("已生成连接器密钥：{}（备份数据时请一并备份）", path.display());
                    hex
                }
            }
        };
        let bytes = from_hex(&key_hex).context("密钥格式应为 64 位十六进制（LEDGER_SECRET_KEY 或 secret.key）")?;
        if bytes.len() != 32 {
            bail!("密钥长度应为 32 字节（64 位十六进制），当前 {} 字节", bytes.len());
        }
        Ok(Self { cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&bytes)) })
    }

    pub fn encrypt(&self, plain: &str) -> anyhow::Result<String> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ct = self.cipher.encrypt(&nonce, plain.as_bytes()).map_err(|_| anyhow::anyhow!("加密失败"))?;
        Ok(to_hex(&[nonce.as_slice(), &ct].concat()))
    }

    pub fn decrypt(&self, hex: &str) -> anyhow::Result<String> {
        let bytes = from_hex(hex)?;
        if bytes.len() < 13 {
            bail!("密文损坏");
        }
        let (nonce, ct) = bytes.split_at(12);
        let plain = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), ct)
            .map_err(|_| anyhow::anyhow!("无法解密已保存的密码（密钥文件与数据不匹配？请重新填写密码）"))?;
        Ok(String::from_utf8(plain)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = Secret::load(dir.path()).unwrap();
        let enc = s.encrypt("p@ss'word中文").unwrap();
        assert_ne!(enc, "p@ss'word中文");
        assert_eq!(s.decrypt(&enc).unwrap(), "p@ss'word中文");
        // 同一明文每次密文不同
        assert_ne!(s.encrypt("x").unwrap(), s.encrypt("x").unwrap());
        // 重新加载同一密钥文件可解密
        let s2 = Secret::load(dir.path()).unwrap();
        assert_eq!(s2.decrypt(&enc).unwrap(), "p@ss'word中文");
        // 篡改后解密失败
        let mut bad = enc.clone();
        bad.replace_range(30..32, if &enc[30..32] == "00" { "11" } else { "00" });
        assert!(s.decrypt(&bad).is_err());
    }
}
