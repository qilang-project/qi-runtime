//! 向量模块 FFI 接口
//!
//! 为 Qi 语言提供 C 接口的向量计算函数

use super::vector::{向量, 向量模块};
use std::sync::OnceLock;

// 全局向量模块实例
static 全局向量模块: OnceLock<向量模块> = OnceLock::new();

fn 获取向量模块() -> &'static 向量模块 {
    全局向量模块.get_or_init(|| 向量模块::创建())
}

/// 初始化向量模块
#[no_mangle]
pub extern "C" fn qi_vector_init() {
    let _ = 获取向量模块();
}

/// 创建向量 - 从数组创建
/// 参数: 数据指针, 长度
/// 返回: 向量ID (用于后续操作)
#[no_mangle]
pub extern "C" fn qi_vector_create(data: *const f64, length: i64) -> i64 {
    if data.is_null() || length <= 0 {
        return -1;
    }

    unsafe {
        let 数据切片 = std::slice::from_raw_parts(data, length as usize);
        let 元素 = 数据切片.to_vec();

        // 简化实现：返回向量长度作为标识
        // 实际实现需要向量池管理
        元素.len() as i64
    }
}

/// 向量点积
/// 参数: v1数据, v1长度, v2数据, v2长度, 结果指针
/// 返回: 0成功, -1失败
#[no_mangle]
pub extern "C" fn qi_vector_dot(
    v1_data: *const f64,
    v1_len: i64,
    v2_data: *const f64,
    v2_len: i64,
    result: *mut f64,
) -> i64 {
    if v1_data.is_null() || v2_data.is_null() || result.is_null() {
        return -1;
    }

    if v1_len != v2_len {
        return -1; // 维度不匹配
    }

    unsafe {
        let v1 = std::slice::from_raw_parts(v1_data, v1_len as usize);
        let v2 = std::slice::from_raw_parts(v2_data, v2_len as usize);

        let 向量1 = 向量 {
            元素: v1.to_vec()
        };
        let 向量2 = 向量 {
            元素: v2.to_vec()
        };

        match 向量1.点积(&向量2) {
            Ok(点积结果) => {
                *result = 点积结果;
                0
            }
            Err(_) => -1,
        }
    }
}

/// 向量加法
/// 参数: v1数据, v1长度, v2数据, v2长度, 结果数据, 结果长度
/// 返回: 0成功, -1失败
#[no_mangle]
pub extern "C" fn qi_vector_add(
    v1_data: *const f64,
    v1_len: i64,
    v2_data: *const f64,
    v2_len: i64,
    result_data: *mut f64,
    result_len: i64,
) -> i64 {
    if v1_data.is_null() || v2_data.is_null() || result_data.is_null() {
        return -1;
    }

    if v1_len != v2_len || result_len < v1_len {
        return -1;
    }

    unsafe {
        let v1 = std::slice::from_raw_parts(v1_data, v1_len as usize);
        let v2 = std::slice::from_raw_parts(v2_data, v2_len as usize);

        let 向量1 = 向量 {
            元素: v1.to_vec()
        };
        let 向量2 = 向量 {
            元素: v2.to_vec()
        };

        match 向量1.加(&向量2) {
            Ok(结果向量) => {
                let result_slice = std::slice::from_raw_parts_mut(result_data, v1_len as usize);
                result_slice.copy_from_slice(&结果向量.元素);
                0
            }
            Err(_) => -1,
        }
    }
}

/// 向量长度(模)
/// 参数: 数据指针, 长度, 结果指针
/// 返回: 0成功, -1失败
#[no_mangle]
pub extern "C" fn qi_vector_magnitude(data: *const f64, length: i64, result: *mut f64) -> i64 {
    if data.is_null() || result.is_null() || length <= 0 {
        return -1;
    }

    unsafe {
        let 数据切片 = std::slice::from_raw_parts(data, length as usize);
        let 向量 = 向量 {
            元素: 数据切片.to_vec(),
        };

        *result = 向量.长度();
        0
    }
}

/// 向量归一化
/// 参数: 输入数据, 长度, 输出数据
/// 返回: 0成功, -1失败
#[no_mangle]
pub extern "C" fn qi_vector_normalize(
    input_data: *const f64,
    length: i64,
    output_data: *mut f64,
) -> i64 {
    if input_data.is_null() || output_data.is_null() || length <= 0 {
        return -1;
    }

    unsafe {
        let 输入 = std::slice::from_raw_parts(input_data, length as usize);
        let 向量 = 向量 {
            元素: 输入.to_vec(),
        };

        match 向量.归一化() {
            Ok(结果) => {
                let output_slice = std::slice::from_raw_parts_mut(output_data, length as usize);
                output_slice.copy_from_slice(&结果.元素);
                0
            }
            Err(_) => -1,
        }
    }
}

/// 余弦相似度 (使用夹角计算: cos = dot / (|v1| * |v2|))
/// 参数: v1数据, v1长度, v2数据, v2长度, 结果指针
/// 返回: 0成功, -1失败
#[no_mangle]
pub extern "C" fn qi_vector_cosine_similarity(
    v1_data: *const f64,
    v1_len: i64,
    v2_data: *const f64,
    v2_len: i64,
    result: *mut f64,
) -> i64 {
    if v1_data.is_null() || v2_data.is_null() || result.is_null() {
        return -1;
    }

    if v1_len != v2_len {
        return -1;
    }

    unsafe {
        let v1 = std::slice::from_raw_parts(v1_data, v1_len as usize);
        let v2 = std::slice::from_raw_parts(v2_data, v2_len as usize);

        let 向量1 = 向量 {
            元素: v1.to_vec()
        };
        let 向量2 = 向量 {
            元素: v2.to_vec()
        };

        // 余弦相似度 = 点积 / (模1 * 模2)
        match 向量1.点积(&向量2) {
            Ok(点积) => {
                let 模1 = 向量1.长度();
                let 模2 = 向量2.长度();
                if 模1 == 0.0 || 模2 == 0.0 {
                    return -1;
                }
                *result = 点积 / (模1 * 模2);
                0
            }
            Err(_) => -1,
        }
    }
}

/// 向量数乘
/// 参数: 输入数据, 长度, 标量, 输出数据
/// 返回: 0成功, -1失败
#[no_mangle]
pub extern "C" fn qi_vector_scale(
    input_data: *const f64,
    length: i64,
    scalar: f64,
    output_data: *mut f64,
) -> i64 {
    if input_data.is_null() || output_data.is_null() || length <= 0 {
        return -1;
    }

    unsafe {
        let 输入 = std::slice::from_raw_parts(input_data, length as usize);
        let 向量 = 向量 {
            元素: 输入.to_vec(),
        };

        let 结果 = 向量.数乘(scalar);
        let output_slice = std::slice::from_raw_parts_mut(output_data, length as usize);
        output_slice.copy_from_slice(&结果.元素);
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_dot_ffi() {
        let v1 = vec![1.0, 2.0, 3.0];
        let v2 = vec![4.0, 5.0, 6.0];
        let mut result: f64 = 0.0;

        let ret = qi_vector_dot(
            v1.as_ptr(),
            v1.len() as i64,
            v2.as_ptr(),
            v2.len() as i64,
            &mut result,
        );

        assert_eq!(ret, 0);
        assert_eq!(result, 32.0); // 1*4 + 2*5 + 3*6 = 32
    }

    #[test]
    fn test_vector_magnitude_ffi() {
        let v = vec![3.0, 4.0];
        let mut result: f64 = 0.0;

        let ret = qi_vector_magnitude(v.as_ptr(), v.len() as i64, &mut result);

        assert_eq!(ret, 0);
        assert_eq!(result, 5.0); // sqrt(9 + 16) = 5
    }

    #[test]
    fn test_vector_cosine_similarity_ffi() {
        let v1 = vec![1.0, 0.0];
        let v2 = vec![0.0, 1.0];
        let mut result: f64 = 0.0;

        let ret = qi_vector_cosine_similarity(
            v1.as_ptr(),
            v1.len() as i64,
            v2.as_ptr(),
            v2.len() as i64,
            &mut result,
        );

        assert_eq!(ret, 0);
        assert!((result - 0.0).abs() < 1e-10); // 垂直向量相似度为0
    }
}
