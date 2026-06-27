//! 向量数学模块 (Vector Mathematics Module)
//!
//! 提供向量化数学计算功能,支持中文接口
//! Provides vectorized mathematical operations with Chinese interface

use std::collections::HashMap;

/// 向量运算错误
#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    #[error("维度不匹配")]
    DimensionMismatch,

    #[error("零向量错误: {0}")]
    ZeroVector(String),

    #[error("索引越界: {0}")]
    IndexOutOfBounds(String),

    #[error("无效操作: {0}")]
    InvalidOperation(String),

    #[error("数值错误: {0}")]
    NumericError(String),
}

pub type VectorResult<T> = Result<T, VectorError>;

/// 向量 (Vector) - 数学向量类型
#[derive(Debug, Clone, PartialEq)]
pub struct 向量 {
    /// 向量元素
    pub 元素: Vec<f64>,
}

impl 向量 {
    /// 创建新向量
    pub fn 创建(元素: Vec<f64>) -> Self {
        Self { 元素 }
    }

    /// 从数组创建
    pub fn 从数组(arr: &[f64]) -> Self {
        Self {
            元素: arr.to_vec()
        }
    }

    /// 零向量
    pub fn 零向量(维度: usize) -> Self {
        Self {
            元素: vec![0.0; 维度],
        }
    }

    /// 单位向量
    pub fn 单位向量(维度: usize, 位置: usize) -> VectorResult<Self> {
        if 位置 >= 维度 {
            return Err(VectorError::InvalidOperation("位置超出维度".to_string()));
        }
        let mut 元素 = vec![0.0; 维度];
        元素[位置] = 1.0;
        Ok(Self { 元素 })
    }

    /// 获取维度
    pub fn 维度(&self) -> usize {
        self.元素.len()
    }

    /// 获取长度(模)
    pub fn 长度(&self) -> f64 {
        self.元素.iter().map(|x| x * x).sum::<f64>().sqrt()
    }

    /// 归一化
    pub fn 归一化(&self) -> VectorResult<Self> {
        let 长度 = self.长度();
        if 长度 == 0.0 {
            return Err(VectorError::InvalidOperation(
                "零向量无法归一化".to_string(),
            ));
        }
        Ok(Self {
            元素: self.元素.iter().map(|x| x / 长度).collect(),
        })
    }

    /// 向量加法
    pub fn 加(&self, 其他: &向量) -> VectorResult<Self> {
        if self.维度() != 其他.维度() {
            return Err(VectorError::InvalidOperation("向量维度不匹配".to_string()));
        }
        Ok(Self {
            元素: self
                .元素
                .iter()
                .zip(其他.元素.iter())
                .map(|(a, b)| a + b)
                .collect(),
        })
    }

    /// 向量减法
    pub fn 减(&self, 其他: &向量) -> VectorResult<Self> {
        if self.维度() != 其他.维度() {
            return Err(VectorError::InvalidOperation("向量维度不匹配".to_string()));
        }
        Ok(Self {
            元素: self
                .元素
                .iter()
                .zip(其他.元素.iter())
                .map(|(a, b)| a - b)
                .collect(),
        })
    }

    /// 数量乘法
    pub fn 数乘(&self, 标量: f64) -> Self {
        Self {
            元素: self.元素.iter().map(|x| x * 标量).collect(),
        }
    }

    /// 点积
    pub fn 点积(&self, 其他: &向量) -> VectorResult<f64> {
        if self.维度() != 其他.维度() {
            return Err(VectorError::InvalidOperation("向量维度不匹配".to_string()));
        }
        Ok(self
            .元素
            .iter()
            .zip(其他.元素.iter())
            .map(|(a, b)| a * b)
            .sum())
    }

    /// 叉积 (仅适用于3维向量)
    pub fn 叉积(&self, 其他: &向量) -> VectorResult<Self> {
        if self.维度() != 3 || 其他.维度() != 3 {
            return Err(VectorError::InvalidOperation(
                "叉积仅支持3维向量".to_string(),
            ));
        }
        let a = &self.元素;
        let b = &其他.元素;
        Ok(Self {
            元素: vec![
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ],
        })
    }

    /// 元素级乘法 (Hadamard积)
    pub fn 元素乘(&self, 其他: &向量) -> VectorResult<Self> {
        if self.维度() != 其他.维度() {
            return Err(VectorError::InvalidOperation("向量维度不匹配".to_string()));
        }
        Ok(Self {
            元素: self
                .元素
                .iter()
                .zip(其他.元素.iter())
                .map(|(a, b)| a * b)
                .collect(),
        })
    }

    /// 元素级除法
    pub fn 元素除(&self, 其他: &向量) -> VectorResult<Self> {
        if self.维度() != 其他.维度() {
            return Err(VectorError::InvalidOperation("向量维度不匹配".to_string()));
        }
        for &值 in &其他.元素 {
            if 值 == 0.0 {
                return Err(VectorError::InvalidOperation("除数不能为零".to_string()));
            }
        }
        Ok(Self {
            元素: self
                .元素
                .iter()
                .zip(其他.元素.iter())
                .map(|(a, b)| a / b)
                .collect(),
        })
    }

    /// 向量投影 (将self投影到other上)
    pub fn 投影(&self, 其他: &向量) -> VectorResult<Self> {
        let 点积 = self.点积(其他)?;
        let 长度平方 = 其他.元素.iter().map(|x| x * x).sum::<f64>();
        if 长度平方 == 0.0 {
            return Err(VectorError::InvalidOperation(
                "无法投影到零向量".to_string(),
            ));
        }
        Ok(其他.数乘(点积 / 长度平方))
    }

    /// 距离
    pub fn 距离(&self, 其他: &向量) -> VectorResult<f64> {
        Ok(self.减(其他)?.长度())
    }

    /// 夹角 (弧度)
    pub fn 夹角(&self, 其他: &向量) -> VectorResult<f64> {
        let 点积 = self.点积(其他)?;
        let 长度积 = self.长度() * 其他.长度();
        if 长度积 == 0.0 {
            return Err(VectorError::InvalidOperation("零向量没有夹角".to_string()));
        }
        Ok((点积 / 长度积).acos())
    }
}

/// 向量数学模块
#[derive(Debug)]
pub struct 向量模块 {
    /// 缓存的常用向量
    常用向量: HashMap<String, 向量>,
}

impl 向量模块 {
    /// 创建新模块
    pub fn 创建() -> Self {
        let mut 模块 = Self {
            常用向量: HashMap::new(),
        };
        模块.初始化常用向量();
        模块
    }

    /// 初始化常用向量
    fn 初始化常用向量(&mut self) {
        // 2D基向量
        self.常用向量
            .insert("单位X".to_string(), 向量::从数组(&[1.0, 0.0]));
        self.常用向量
            .insert("单位Y".to_string(), 向量::从数组(&[0.0, 1.0]));

        // 3D基向量
        self.常用向量
            .insert("单位X3D".to_string(), 向量::从数组(&[1.0, 0.0, 0.0]));
        self.常用向量
            .insert("单位Y3D".to_string(), 向量::从数组(&[0.0, 1.0, 0.0]));
        self.常用向量
            .insert("单位Z3D".to_string(), 向量::从数组(&[0.0, 0.0, 1.0]));
    }

    /// 获取常用向量
    pub fn 获取常用向量(&self, 名称: &str) -> Option<&向量> {
        self.常用向量.get(名称)
    }

    /// 向量化运算 - 对数组中每个元素应用函数
    pub fn 向量化(&self, 数据: &[f64], 函数: fn(f64) -> f64) -> Vec<f64> {
        数据.iter().map(|&x| 函数(x)).collect()
    }

    /// 向量化 - 平方根
    pub fn 向量化平方根(&self, 数据: &[f64]) -> VectorResult<Vec<f64>> {
        let mut 结果 = Vec::with_capacity(数据.len());
        for &值 in 数据 {
            if 值 < 0.0 {
                return Err(VectorError::InvalidOperation(
                    "负数没有实数平方根".to_string(),
                ));
            }
            结果.push(值.sqrt());
        }
        Ok(结果)
    }

    /// 向量化 - 正弦
    pub fn 向量化正弦(&self, 数据: &[f64]) -> Vec<f64> {
        数据.iter().map(|&x| x.sin()).collect()
    }

    /// 向量化 - 余弦
    pub fn 向量化余弦(&self, 数据: &[f64]) -> Vec<f64> {
        数据.iter().map(|&x| x.cos()).collect()
    }

    /// 向量化 - 指数
    pub fn 向量化指数(&self, 数据: &[f64]) -> Vec<f64> {
        数据.iter().map(|&x| x.exp()).collect()
    }

    /// 向量化 - 自然对数
    pub fn 向量化对数(&self, 数据: &[f64]) -> VectorResult<Vec<f64>> {
        let mut 结果 = Vec::with_capacity(数据.len());
        for &值 in 数据 {
            if 值 <= 0.0 {
                return Err(VectorError::InvalidOperation(
                    "对数函数的参数必须为正数".to_string(),
                ));
            }
            结果.push(值.ln());
        }
        Ok(结果)
    }

    /// 向量化 - 幂运算
    pub fn 向量化幂(&self, 数据: &[f64], 指数: f64) -> Vec<f64> {
        数据.iter().map(|&x| x.powf(指数)).collect()
    }

    /// 向量化 - 绝对值
    pub fn 向量化绝对值(&self, 数据: &[f64]) -> Vec<f64> {
        数据.iter().map(|&x| x.abs()).collect()
    }

    /// 聚合函数 - 求和
    pub fn 求和(&self, 数据: &[f64]) -> f64 {
        数据.iter().sum()
    }

    /// 聚合函数 - 求积
    pub fn 求积(&self, 数据: &[f64]) -> f64 {
        数据.iter().product()
    }

    /// 聚合函数 - 平均值
    pub fn 平均值(&self, 数据: &[f64]) -> VectorResult<f64> {
        if 数据.is_empty() {
            return Err(VectorError::InvalidOperation(
                "空数组没有平均值".to_string(),
            ));
        }
        Ok(self.求和(数据) / 数据.len() as f64)
    }

    /// 聚合函数 - 最大值
    pub fn 最大值(&self, 数据: &[f64]) -> VectorResult<f64> {
        数据
            .iter()
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .copied()
            .ok_or_else(|| VectorError::InvalidOperation("空数组没有最大值".to_string()))
    }

    /// 聚合函数 - 最小值
    pub fn 最小值(&self, 数据: &[f64]) -> VectorResult<f64> {
        数据
            .iter()
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .copied()
            .ok_or_else(|| VectorError::InvalidOperation("空数组没有最小值".to_string()))
    }

    /// 统计函数 - 方差
    pub fn 方差(&self, 数据: &[f64]) -> VectorResult<f64> {
        let 平均 = self.平均值(数据)?;
        let 平方和 = 数据.iter().map(|x| (x - 平均).powi(2)).sum::<f64>();
        Ok(平方和 / 数据.len() as f64)
    }

    /// 统计函数 - 标准差
    pub fn 标准差(&self, 数据: &[f64]) -> VectorResult<f64> {
        Ok(self.方差(数据)?.sqrt())
    }
}

impl Default for 向量模块 {
    fn default() -> Self {
        Self::创建()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 测试向量创建() {
        let v = 向量::从数组(&[1.0, 2.0, 3.0]);
        assert_eq!(v.维度(), 3);
        assert_eq!(v.元素, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn 测试向量加法() {
        let v1 = 向量::从数组(&[1.0, 2.0]);
        let v2 = 向量::从数组(&[3.0, 4.0]);
        let 结果 = v1.加(&v2).unwrap();
        assert_eq!(结果.元素, vec![4.0, 6.0]);
    }

    #[test]
    fn 测试点积() {
        let v1 = 向量::从数组(&[1.0, 2.0, 3.0]);
        let v2 = 向量::从数组(&[4.0, 5.0, 6.0]);
        let 结果 = v1.点积(&v2).unwrap();
        assert_eq!(结果, 32.0); // 1*4 + 2*5 + 3*6 = 32
    }

    #[test]
    fn 测试向量化运算() {
        let 模块 = 向量模块::创建();
        let 数据 = vec![1.0, 4.0, 9.0, 16.0];
        let 结果 = 模块.向量化平方根(&数据).unwrap();
        assert_eq!(结果, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn 测试统计函数() {
        let 模块 = 向量模块::创建();
        let 数据 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(模块.求和(&数据), 15.0);
        assert_eq!(模块.平均值(&数据).unwrap(), 3.0);
        assert_eq!(模块.最大值(&数据).unwrap(), 5.0);
        assert_eq!(模块.最小值(&数据).unwrap(), 1.0);
    }
}
