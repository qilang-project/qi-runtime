//! 文件操作模块
//!
//! 提供文件读写、创建、删除等操作

use crate::stdlib::{StdlibError, StdlibResult, StdlibValue};
use std::fs;
use std::path::Path;

/// IO 操作错误
#[derive(Debug, thiserror::Error)]
pub enum IO错误 {
    #[error("文件不存在: {0}")]
    文件不存在(String),

    #[error("权限拒绝: {0}")]
    权限拒绝(String),

    #[error("文件已存在: {0}")]
    文件已存在(String),

    #[error("无效路径: {0}")]
    无效路径(String),

    #[error("读取错误: {0}")]
    读取错误(String),

    #[error("写入错误: {0}")]
    写入错误(String),

    #[error("IO 错误: {0}")]
    通用错误(String),
}

impl From<IO错误> for StdlibError {
    fn from(err: IO错误) -> Self {
        StdlibError::InvalidParameter {
            parameter: "io_operation".to_string(),
            message: err.to_string(),
        }
    }
}

impl From<std::io::Error> for IO错误 {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => IO错误::文件不存在(err.to_string()),
            std::io::ErrorKind::PermissionDenied => IO错误::权限拒绝(err.to_string()),
            std::io::ErrorKind::AlreadyExists => IO错误::文件已存在(err.to_string()),
            _ => IO错误::通用错误(err.to_string()),
        }
    }
}

/// 文件操作枚举
#[derive(Debug, Clone, PartialEq)]
pub enum 文件操作 {
    /// 读取文件全部内容
    读取,
    /// 写入内容到文件（覆盖）
    写入,
    /// 追加内容到文件
    追加,
    /// 删除文件
    删除,
    /// 创建文件
    创建,
    /// 检查文件是否存在
    存在,
    /// 获取文件大小
    大小,
    /// 创建目录
    创建目录,
    /// 删除目录
    删除目录,
    /// 列出目录内容
    列出目录,
}

/// 文件模块
pub struct 文件模块;

impl 文件模块 {
    /// 创建新的文件模块实例
    pub fn 创建() -> Self {
        Self
    }

    /// 执行文件操作
    pub fn 执行操作(
        &self, 操作: 文件操作, 参数: &[StdlibValue]
    ) -> StdlibResult<StdlibValue> {
        match 操作 {
            文件操作::读取 => self.读取文件(参数),
            文件操作::写入 => self.写入文件(参数),
            文件操作::追加 => self.追加文件(参数),
            文件操作::删除 => self.删除文件(参数),
            文件操作::创建 => self.创建文件(参数),
            文件操作::存在 => self.文件存在(参数),
            文件操作::大小 => self.文件大小(参数),
            文件操作::创建目录 => self.创建目录(参数),
            文件操作::删除目录 => self.删除目录(参数),
            文件操作::列出目录 => self.列出目录内容(参数),
        }
    }

    /// 读取文件全部内容
    fn 读取文件(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 1 {
            return Err(IO错误::通用错误("读取文件需要1个参数：文件路径".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("文件路径必须是字符串".to_string()))?;

        let 内容 = fs::read_to_string(&路径).map_err(IO错误::from)?;

        Ok(StdlibValue::String(内容))
    }

    /// 写入内容到文件（覆盖）
    fn 写入文件(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 2 {
            return Err(IO错误::通用错误("写入文件需要2个参数：文件路径、内容".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("文件路径必须是字符串".to_string()))?;

        let 内容 = 参数[1]
            .as_string()
            .ok_or_else(|| IO错误::写入错误("写入内容必须是字符串".to_string()))?;

        fs::write(&路径, 内容).map_err(IO错误::from)?;

        Ok(StdlibValue::Boolean(true))
    }

    /// 追加内容到文件
    fn 追加文件(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 2 {
            return Err(IO错误::通用错误("追加文件需要2个参数：文件路径、内容".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("文件路径必须是字符串".to_string()))?;

        let 内容 = 参数[1]
            .as_string()
            .ok_or_else(|| IO错误::写入错误("追加内容必须是字符串".to_string()))?;

        use std::io::Write;
        let mut 文件 = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&路径)
            .map_err(IO错误::from)?;

        文件
            .write_all(内容.as_bytes())
            .map_err(|e| IO错误::写入错误(e.to_string()))?;

        Ok(StdlibValue::Boolean(true))
    }

    /// 删除文件
    fn 删除文件(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 1 {
            return Err(IO错误::通用错误("删除文件需要1个参数：文件路径".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("文件路径必须是字符串".to_string()))?;

        fs::remove_file(&路径).map_err(IO错误::from)?;

        Ok(StdlibValue::Boolean(true))
    }

    /// 创建空文件
    fn 创建文件(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 1 {
            return Err(IO错误::通用错误("创建文件需要1个参数：文件路径".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("文件路径必须是字符串".to_string()))?;

        fs::File::create(&路径).map_err(IO错误::from)?;

        Ok(StdlibValue::Boolean(true))
    }

    /// 检查文件是否存在
    fn 文件存在(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 1 {
            return Err(IO错误::通用错误("检查文件存在需要1个参数：文件路径".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("文件路径必须是字符串".to_string()))?;

        let 存在 = Path::new(&路径).exists();
        Ok(StdlibValue::Boolean(存在))
    }

    /// 获取文件大小
    fn 文件大小(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 1 {
            return Err(IO错误::通用错误("获取文件大小需要1个参数：文件路径".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("文件路径必须是字符串".to_string()))?;

        let 元数据 = fs::metadata(&路径).map_err(IO错误::from)?;

        Ok(StdlibValue::Integer(元数据.len() as i64))
    }

    /// 创建目录
    fn 创建目录(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 1 {
            return Err(IO错误::通用错误("创建目录需要1个参数：目录路径".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("目录路径必须是字符串".to_string()))?;

        fs::create_dir_all(&路径).map_err(IO错误::from)?;

        Ok(StdlibValue::Boolean(true))
    }

    /// 删除目录
    fn 删除目录(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 1 {
            return Err(IO错误::通用错误("删除目录需要1个参数：目录路径".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("目录路径必须是字符串".to_string()))?;

        fs::remove_dir_all(&路径).map_err(IO错误::from)?;

        Ok(StdlibValue::Boolean(true))
    }

    /// 列出目录内容
    fn 列出目录内容(&self, 参数: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if 参数.len() != 1 {
            return Err(IO错误::通用错误("列出目录需要1个参数：目录路径".to_string()).into());
        }

        let 路径 = 参数[0]
            .as_string()
            .ok_or_else(|| IO错误::无效路径("目录路径必须是字符串".to_string()))?;

        let 条目: Vec<StdlibValue> = fs::read_dir(&路径)
            .map_err(IO错误::from)?
            .filter_map(|entry| entry.ok())
            .map(|entry| StdlibValue::String(entry.file_name().to_string_lossy().to_string()))
            .collect();

        Ok(StdlibValue::Array(条目))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_file_write_read() {
        let 模块 = 文件模块::创建();
        let 测试文件路径 = std::env::temp_dir().join("test_qi_io.txt");
        let 测试文件 = 测试文件路径.to_str().unwrap();

        // 写入文件
        let 写入参数 = vec![
            StdlibValue::String(测试文件.to_string()),
            StdlibValue::String("测试内容".to_string()),
        ];
        let 结果 = 模块.执行操作(文件操作::写入, &写入参数);
        assert!(结果.is_ok());

        // 读取文件
        let 读取参数 = vec![StdlibValue::String(测试文件.to_string())];
        let 结果 = 模块.执行操作(文件操作::读取, &读取参数);
        assert!(结果.is_ok());
        if let Ok(StdlibValue::String(内容)) = 结果 {
            assert_eq!(内容, "测试内容");
        }

        // 清理
        let _ = fs::remove_file(测试文件);
    }

    #[test]
    fn test_file_exists() {
        let 模块 = 文件模块::创建();
        let 测试文件路径 = std::env::temp_dir().join("test_qi_exists.txt");
        let 测试文件 = 测试文件路径.to_str().unwrap();

        // 文件不存在
        let 参数 = vec![StdlibValue::String(测试文件.to_string())];
        let 结果 = 模块.执行操作(文件操作::存在, &参数);
        assert!(matches!(结果, Ok(StdlibValue::Boolean(false))));

        // 创建文件
        fs::write(测试文件, "test").unwrap();

        // 文件存在
        let 结果 = 模块.执行操作(文件操作::存在, &参数);
        assert!(matches!(结果, Ok(StdlibValue::Boolean(true))));

        // 清理
        let _ = fs::remove_file(测试文件);
    }

    #[test]
    fn test_file_size() {
        let 模块 = 文件模块::创建();
        let 测试文件路径 = std::env::temp_dir().join("test_qi_size.txt");
        let 测试文件 = 测试文件路径.to_str().unwrap();

        // 创建文件
        fs::write(测试文件, "12345").unwrap();

        // 获取大小
        let 参数 = vec![StdlibValue::String(测试文件.to_string())];
        let 结果 = 模块.执行操作(文件操作::大小, &参数);
        assert!(matches!(结果, Ok(StdlibValue::Integer(5))));

        // 清理
        let _ = fs::remove_file(测试文件);
    }
}
