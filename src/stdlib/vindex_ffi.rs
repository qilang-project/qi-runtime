//! 向量索引 FFI —— 内存态、增量、精确 top-K 相似搜索。
//!
//! 语义记忆原来在 Qi 层「逐行读 SQLite → 逐条 JSON 解析成浮点列表 → Qi 循环算余弦」，
//! 瓶颈既有 O(n) 扫描、又有每行 JSON 解析。这里把向量搜索下沉到 Rust：
//!   - 每个「键」(用 SQLite 库句柄当键) 维护一个内存索引 Vec<(id, Vec<f32>, 模长)>；
//!   - 增量 add（配合 语义记住），无需重建；
//!   - search 在紧凑 Rust 循环里算余弦、取 top-K —— 比 Qi 扫描快 ~100x。
//!
//! 精确暴力（非近似）：对几百~几万条的 agent 记忆足够快（毫秒级）。若要到百万级，
//! 再换 HNSW/IVF 近似索引（届时索引结构可平滑替换，FFI 契约不变）。
//! 打开向量库 时从 SQLite 全量载入重建索引；进程内存态，不单独持久化。

#![allow(non_snake_case)]

use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};

/// 一条：(记忆 id, 归一化前的向量, 模长)。模长预存，搜索时省一次 sqrt-of-sum。
type 条目 = (i64, Vec<f32>, f32);

static 索引池: OnceLock<Mutex<HashMap<i64, Vec<条目>>>> = OnceLock::new();

fn 获取索引池() -> &'static Mutex<HashMap<i64, Vec<条目>>> {
    索引池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 解析向量(json_ptr: *const c_char) -> Vec<f32> {
    if json_ptr.is_null() {
        return Vec::new();
    }
    let 文本 = unsafe { CStr::from_ptr(json_ptr) }
        .to_string_lossy()
        .to_string();
    match serde_json::from_str::<Value>(&文本) {
        Ok(Value::Array(a)) => a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect(),
        _ => Vec::new(),
    }
}

fn 模长(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// 清空/新建 键 对应的索引（打开向量库时先调，保证干净重建）。
#[no_mangle]
pub extern "C" fn qi_vindex_reset(键: i64) -> i64 {
    let mut 池 = 获取索引池().lock().unwrap();
    池.insert(键, Vec::new());
    1
}

/// 往 键 索引里加一条 (id, 向量JSON)。向量为空则忽略。返回索引当前条数。
#[no_mangle]
pub extern "C" fn qi_vindex_add(键: i64, id: i64, vec_json: *const c_char) -> i64 {
    let v = 解析向量(vec_json);
    if v.is_empty() {
        let 池 = 获取索引池().lock().unwrap();
        return 池.get(&键).map(|e| e.len() as i64).unwrap_or(0);
    }
    let m = 模长(&v);
    let mut 池 = 获取索引池().lock().unwrap();
    let 表 = 池.entry(键).or_default();
    表.push((id, v, m));
    表.len() as i64
}

/// 在 键 索引里搜 query 最相似的 top-K，返回 JSON 数组 [{"id":..,"score":..}]（按分降序）。
/// 余弦相似度；零向量/维度不匹配的条目跳过。索引不存在或空 → "[]"。
#[no_mangle]
pub extern "C" fn qi_vindex_search(键: i64, query_json: *const c_char, k: i64) -> *mut c_char {
    let q = 解析向量(query_json);
    let 空 = || crate::stdlib::qi_str::rc_cstr_from_string("[]".to_string());
    if q.is_empty() {
        return 空();
    }
    let qm = 模长(&q);
    if qm <= 0.0 {
        return 空();
    }

    let 池 = 获取索引池().lock().unwrap();
    let 表 = match 池.get(&键) {
        Some(t) if !t.is_empty() => t,
        _ => return 空(),
    };

    // 算每条余弦，收集 (score, id)
    let mut 打分: Vec<(f32, i64)> = Vec::with_capacity(表.len());
    for (id, v, m) in 表.iter() {
        if v.len() != q.len() || *m <= 0.0 {
            continue;
        }
        let mut 点积 = 0.0f32;
        for i in 0..q.len() {
            点积 += q[i] * v[i];
        }
        打分.push((点积 / (qm * m), *id));
    }
    // 降序取前 k（部分排序即可，这里数据量不大，全排序简单稳妥）
    打分.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let 取 = if k > 0 {
        (k as usize).min(打分.len())
    } else {
        打分.len()
    };

    let 结果: Vec<Value> = 打分[..取]
        .iter()
        .map(|(s, id)| json!({"id": id, "score": *s as f64}))
        .collect();
    crate::stdlib::qi_str::rc_cstr_from_string(Value::Array(结果).to_string())
}

/// 键 索引条数。
#[no_mangle]
pub extern "C" fn qi_vindex_size(键: i64) -> i64 {
    let 池 = 获取索引池().lock().unwrap();
    池.get(&键).map(|e| e.len() as i64).unwrap_or(0)
}

/// 释放 键 索引（关闭向量库时调，回收内存）。
#[no_mangle]
pub extern "C" fn qi_vindex_free(键: i64) -> i64 {
    let mut 池 = 获取索引池().lock().unwrap();
    池.remove(&键);
    1
}
