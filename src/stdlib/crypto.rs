//! 加密模块
//!
//! 本模块提供加密功能，包括哈希、编码和消息认证码操作。

use super::{StdlibError, StdlibResult, StdlibValue};
use md5::Md5;
use sha2::{Digest, Sha256, Sha512};

/// 加密操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum 加密操作 {
    /// MD5 哈希
    MD5哈希,
    /// SHA256 哈希
    SHA256哈希,
    /// SHA512 哈希
    SHA512哈希,
    /// Base64 编码
    Base64编码,
    /// Base64 解码
    Base64解码,
    /// HMAC-SHA256 消息认证码
    HMAC_SHA256,
    /// HMAC-SHA512 消息认证码
    HMAC_SHA512,
}

/// 加密模块
#[derive(Debug)]
pub struct 加密模块 {
    /// 编码格式（十六进制或Base64）
    编码格式: 编码格式,
}

/// 哈希输出的编码格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum 编码格式 {
    /// 十六进制编码
    十六进制,
    /// Base64 编码
    Base64,
}

impl 加密模块 {
    /// 创建新的加密模块
    pub fn 创建() -> Self {
        Self {
            编码格式: 编码格式::十六进制,
        }
    }

    /// 使用指定编码格式创建加密模块
    pub fn 使用编码格式(编码格式: 编码格式) -> Self {
        Self { 编码格式 }
    }

    /// 初始化加密模块
    pub fn 初始化(&mut self) -> StdlibResult<()> {
        Ok(())
    }

    /// 设置编码格式
    pub fn 设置编码格式(&mut self, 格式: 编码格式) {
        self.编码格式 = 格式;
    }

    /// 执行加密操作
    pub fn 执行操作(
        &self, 操作: 加密操作, 参数: &[StdlibValue]
    ) -> StdlibResult<StdlibValue> {
        match 操作 {
            加密操作::MD5哈希 => self.md5哈希(参数),
            加密操作::SHA256哈希 => self.sha256哈希(参数),
            加密操作::SHA512哈希 => self.sha512哈希(参数),
            加密操作::Base64编码 => self.base64编码(参数),
            加密操作::Base64解码 => self.base64解码(参数),
            加密操作::HMAC_SHA256 => self.hmac_sha256(参数),
            加密操作::HMAC_SHA512 => self.hmac_sha512(参数),
        }
    }

    /// 计算 MD5 哈希
    fn md5哈希(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.is_empty() {
            return Err(StdlibError::CryptoError {
                operation: "md5哈希".to_string(),
                message: "需要字符串参数".to_string(),
            });
        }

        let 输入 = match &参数[0] {
            StdlibValue::String(s) => s.as_bytes(),
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "md5哈希".to_string(),
                    message: "参数必须是字符串".to_string(),
                });
            }
        };

        let mut 哈希器 = Md5::new();
        哈希器.update(输入);
        let 结果 = 哈希器.finalize();

        let 哈希值 = match self.编码格式 {
            编码格式::十六进制 => format!("{:x}", 结果),
            编码格式::Base64 => {
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, 结果)
            }
        };

        Ok(StdlibValue::String(哈希值))
    }

    /// 计算 SHA256 哈希
    fn sha256哈希(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.is_empty() {
            return Err(StdlibError::CryptoError {
                operation: "sha256哈希".to_string(),
                message: "需要字符串参数".to_string(),
            });
        }

        let 输入 = match &参数[0] {
            StdlibValue::String(s) => s.as_bytes(),
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "sha256哈希".to_string(),
                    message: "参数必须是字符串".to_string(),
                });
            }
        };

        let mut 哈希器 = Sha256::new();
        哈希器.update(输入);
        let 结果 = 哈希器.finalize();

        let 哈希值 = match self.编码格式 {
            编码格式::十六进制 => format!("{:x}", 结果),
            编码格式::Base64 => {
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, 结果)
            }
        };

        Ok(StdlibValue::String(哈希值))
    }

    /// 计算 SHA512 哈希
    fn sha512哈希(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.is_empty() {
            return Err(StdlibError::CryptoError {
                operation: "sha512哈希".to_string(),
                message: "需要字符串参数".to_string(),
            });
        }

        let 输入 = match &参数[0] {
            StdlibValue::String(s) => s.as_bytes(),
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "sha512哈希".to_string(),
                    message: "参数必须是字符串".to_string(),
                });
            }
        };

        let mut 哈希器 = Sha512::new();
        哈希器.update(输入);
        let 结果 = 哈希器.finalize();

        let 哈希值 = match self.编码格式 {
            编码格式::十六进制 => format!("{:x}", 结果),
            编码格式::Base64 => {
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, 结果)
            }
        };

        Ok(StdlibValue::String(哈希值))
    }

    /// Base64 编码
    fn base64编码(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.is_empty() {
            return Err(StdlibError::CryptoError {
                operation: "base64编码".to_string(),
                message: "需要字符串参数".to_string(),
            });
        }

        let 输入 = match &参数[0] {
            StdlibValue::String(s) => s.as_bytes(),
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "base64编码".to_string(),
                    message: "参数必须是字符串".to_string(),
                });
            }
        };

        let 编码结果 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, 输入);
        Ok(StdlibValue::String(编码结果))
    }

    /// Base64 解码
    fn base64解码(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.is_empty() {
            return Err(StdlibError::CryptoError {
                operation: "base64解码".to_string(),
                message: "需要字符串参数".to_string(),
            });
        }

        let 输入 = match &参数[0] {
            StdlibValue::String(s) => s,
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "base64解码".to_string(),
                    message: "参数必须是字符串".to_string(),
                });
            }
        };

        let 解码字节 = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, 输入)
            .map_err(|e| StdlibError::CryptoError {
                operation: "base64解码".to_string(),
                message: format!("Base64解码失败: {}", e),
            })?;

        let 结果 = String::from_utf8(解码字节).map_err(|e| StdlibError::CryptoError {
            operation: "base64解码".to_string(),
            message: format!("UTF-8转换失败: {}", e),
        })?;

        Ok(StdlibValue::String(结果))
    }

    /// 计算 HMAC-SHA256 消息认证码
    fn hmac_sha256(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() < 2 {
            return Err(StdlibError::CryptoError {
                operation: "hmac_sha256".to_string(),
                message: "需要消息和密钥两个参数".to_string(),
            });
        }

        let 消息 = match &参数[0] {
            StdlibValue::String(s) => s.as_bytes(),
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "hmac_sha256".to_string(),
                    message: "第一个参数（消息）必须是字符串".to_string(),
                });
            }
        };

        let 密钥 = match &参数[1] {
            StdlibValue::String(s) => s.as_bytes(),
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "hmac_sha256".to_string(),
                    message: "第二个参数（密钥）必须是字符串".to_string(),
                });
            }
        };

        use hmac::{Hmac, Mac};
        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(密钥).map_err(|e| StdlibError::CryptoError {
            operation: "hmac_sha256".to_string(),
            message: format!("HMAC初始化失败: {}", e),
        })?;

        mac.update(消息);
        let 结果 = mac.finalize();
        let 认证码字节 = 结果.into_bytes();

        let 哈希值 = match self.编码格式 {
            编码格式::十六进制 => format!("{:x}", 认证码字节),
            编码格式::Base64 => {
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, 认证码字节)
            }
        };

        Ok(StdlibValue::String(哈希值))
    }

    /// 计算 HMAC-SHA512 消息认证码
    fn hmac_sha512(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() < 2 {
            return Err(StdlibError::CryptoError {
                operation: "hmac_sha512".to_string(),
                message: "需要消息和密钥两个参数".to_string(),
            });
        }

        let 消息 = match &参数[0] {
            StdlibValue::String(s) => s.as_bytes(),
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "hmac_sha512".to_string(),
                    message: "第一个参数（消息）必须是字符串".to_string(),
                });
            }
        };

        let 密钥 = match &参数[1] {
            StdlibValue::String(s) => s.as_bytes(),
            _ => {
                return Err(StdlibError::CryptoError {
                    operation: "hmac_sha512".to_string(),
                    message: "第二个参数（密钥）必须是字符串".to_string(),
                });
            }
        };

        use hmac::{Hmac, Mac};
        type HmacSha512 = Hmac<Sha512>;

        let mut mac = HmacSha512::new_from_slice(密钥).map_err(|e| StdlibError::CryptoError {
            operation: "hmac_sha512".to_string(),
            message: format!("HMAC初始化失败: {}", e),
        })?;

        mac.update(消息);
        let 结果 = mac.finalize();
        let 认证码字节 = 结果.into_bytes();

        let 哈希值 = match self.编码格式 {
            编码格式::十六进制 => format!("{:x}", 认证码字节),
            编码格式::Base64 => {
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, 认证码字节)
            }
        };

        Ok(StdlibValue::String(哈希值))
    }
}

impl Default for 加密模块 {
    fn default() -> Self {
        Self::创建()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_md5哈希() {
        let 加密 = 加密模块::创建();
        let 参数 = vec![StdlibValue::String("hello".to_string())];
        let 结果 = 加密.md5哈希(&参数).unwrap();

        match 结果 {
            StdlibValue::String(哈希值) => {
                assert_eq!(哈希值, "5d41402abc4b2a76b9719d911017c592");
            }
            _ => panic!("期望字符串结果"),
        }
    }

    #[test]
    fn test_sha256哈希() {
        let 加密 = 加密模块::创建();
        let 参数 = vec![StdlibValue::String("hello".to_string())];
        let 结果 = 加密.sha256哈希(&参数).unwrap();

        match 结果 {
            StdlibValue::String(哈希值) => {
                assert_eq!(
                    哈希值,
                    "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                );
            }
            _ => panic!("期望字符串结果"),
        }
    }

    #[test]
    fn test_sha512哈希() {
        let 加密 = 加密模块::创建();
        let 参数 = vec![StdlibValue::String("hello".to_string())];
        let 结果 = 加密.sha512哈希(&参数).unwrap();

        match 结果 {
            StdlibValue::String(哈希值) => {
                assert!(哈希值.starts_with("9b71d224bd62f3785d96d46ad3ea3d73"));
            }
            _ => panic!("期望字符串结果"),
        }
    }

    #[test]
    fn test_base64编码解码() {
        let 加密 = 加密模块::创建();

        // 测试编码
        let 参数 = vec![StdlibValue::String("hello world".to_string())];
        let 编码结果 = 加密.base64编码(&参数).unwrap();

        match 编码结果 {
            StdlibValue::String(s) => {
                assert_eq!(s, "aGVsbG8gd29ybGQ=");

                // 测试解码
                let 解码参数 = vec![StdlibValue::String(s)];
                let 解码结果 = 加密.base64解码(&解码参数).unwrap();

                match 解码结果 {
                    StdlibValue::String(原文) => {
                        assert_eq!(原文, "hello world");
                    }
                    _ => panic!("期望字符串结果"),
                }
            }
            _ => panic!("期望字符串结果"),
        }
    }

    #[test]
    fn test_hmac_sha256() {
        let 加密 = 加密模块::创建();
        let 参数 = vec![
            StdlibValue::String("message".to_string()),
            StdlibValue::String("secret-key".to_string()),
        ];
        let 结果 = 加密.hmac_sha256(&参数).unwrap();

        match 结果 {
            StdlibValue::String(哈希值) => {
                // 验证是否为有效的十六进制字符串（SHA256 = 64个十六进制字符）
                assert_eq!(哈希值.len(), 64);
                assert!(哈希值.chars().all(|c| c.is_ascii_hexdigit()));
            }
            _ => panic!("期望字符串结果"),
        }
    }

    #[test]
    fn test_加密模块创建() {
        let 加密 = 加密模块::创建();
        assert_eq!(加密.编码格式, 编码格式::十六进制);

        let 加密_base64 = 加密模块::使用编码格式(编码格式::Base64);
        assert_eq!(加密_base64.编码格式, 编码格式::Base64);
    }

    #[test]
    fn test_中文文本哈希() {
        let 加密 = 加密模块::创建();
        let 参数 = vec![StdlibValue::String("你好世界".to_string())];
        let 结果 = 加密.sha256哈希(&参数).unwrap();

        match 结果 {
            StdlibValue::String(哈希值) => {
                // 验证中文文本的哈希计算正确
                assert_eq!(哈希值.len(), 64);
                assert!(哈希值.chars().all(|c| c.is_ascii_hexdigit()));
            }
            _ => panic!("期望字符串结果"),
        }
    }
}
