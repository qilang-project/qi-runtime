//! 词法索引 FFI —— 内存态、增量、BM25 top-K 词法检索（与 vindex_ffi 成对，做混合检索）。
//!
//! 向量检索（vindex）擅长语义相近但用词不同的召回；反过来「精确关键词/代号/型号」
//! （如 QX-9000）语义平淡时向量常常漏，词法检索必中。两路各有强弱 → 上层 RRF 融合。
//!
//! 中文没有空格分词，也不想引入分词器依赖：
//!   - CJK（及一切非 ASCII 的字母/数字）用 **单字 + 相邻字符 bigram** 作 term：
//!       "混合检索" → [混, 合, 检, 索, 混合, 合检, 检索]
//!     bigram 提供短语区分度，单字兜底召回，对中文检索足够好且零依赖；
//!   - ASCII 按 空白/标点 切词、小写化："QX-9000" → [qx, 9000]；
//!   - 标点/空白打断 bigram 连续性（"你好，世界" 不产生 "好世"）。
//!
//! BM25 标准公式（k1=1.2, b=0.75），IDF 用 ln(1 + (N-df+0.5)/(df+0.5))（非负）。
//! 结构与 vindex_ffi 一致：全局池按 键 分索引，reset/add/search/size/free；
//! search 返回 JSON 数组 [{"id":..,"score":..}]（按分降序），与 qi_vindex_search 同风格。

#![allow(non_snake_case)]

use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};

const K1: f64 = 1.2;
const B: f64 = 0.75;

/// 一个键对应的 BM25 索引：文档表 + 倒排表（term → [(文档槽位, 词频)]）。
#[derive(Default)]
struct 索引 {
    /// 每篇：(外部 id, 文档 token 总数)。槽位 = Vec 下标，append-only。
    文档: Vec<(i64, u32)>,
    /// term → 倒排链 [(槽位, tf)]。add 追加即可，无需重建。
    倒排: HashMap<String, Vec<(u32, u32)>>,
    /// 所有文档 token 总数（求平均长度用）。
    总词数: u64,
}

static 索引池: OnceLock<Mutex<HashMap<i64, 索引>>> = OnceLock::new();

fn 获取索引池() -> &'static Mutex<HashMap<i64, 索引>> {
    索引池.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 切词：CJK 单字 + bigram；ASCII 字母数字连成小写词；其余字符作分隔并打断 bigram。
fn 切词(文本: &str) -> Vec<String> {
    let mut 词表 = Vec::new();
    let mut ascii词 = String::new();
    let mut 前一字: Option<char> = None;
    for c in 文本.chars() {
        if c.is_ascii_alphanumeric() {
            // ASCII 词内字符：累积、小写化；同时打断 CJK bigram
            ascii词.push(c.to_ascii_lowercase());
            前一字 = None;
        } else {
            if !ascii词.is_empty() {
                词表.push(std::mem::take(&mut ascii词));
            }
            if c.is_alphabetic() || c.is_numeric() {
                // 非 ASCII 的字母/数字（中文、全角等）：单字 term
                词表.push(c.to_string());
                if let Some(p) = 前一字 {
                    let mut 双 = String::with_capacity(8);
                    双.push(p);
                    双.push(c);
                    词表.push(双);
                }
                前一字 = Some(c);
            } else {
                // 标点/空白：打断 bigram
                前一字 = None;
            }
        }
    }
    if !ascii词.is_empty() {
        词表.push(ascii词);
    }
    词表
}

/// 纯逻辑加文档（供 FFI 和单测共用）。返回加后文档数。
fn 索引加文档(索: &mut 索引, id: i64, 文本: &str) -> usize {
    let 词表 = 切词(文本);
    let 槽位 = 索.文档.len() as u32;
    let mut 词频: HashMap<String, u32> = HashMap::new();
    for 词 in &词表 {
        *词频.entry(词.clone()).or_insert(0) += 1;
    }
    索.文档.push((id, 词表.len() as u32));
    索.总词数 += 词表.len() as u64;
    for (词, tf) in 词频 {
        索.倒排.entry(词).or_default().push((槽位, tf));
    }
    索.文档.len()
}

/// 纯逻辑 BM25 搜索（供 FFI 和单测共用）。返回 [(id, score)] 按分降序，最多 k 条。
fn 索引搜索(索: &索引, 查询: &str, k: usize) -> Vec<(i64, f64)> {
    let n = 索.文档.len();
    if n == 0 {
        return Vec::new();
    }
    let 平均长度 = 索.总词数 as f64 / n as f64;

    // 查询 term 去重（BM25 对重复查询词不重复计分即可，简单稳妥）
    let mut 查询词 = 切词(查询);
    查询词.sort();
    查询词.dedup();

    let mut 得分: HashMap<u32, f64> = HashMap::new();
    for 词 in &查询词 {
        let 链 = match 索.倒排.get(词) {
            Some(l) => l,
            None => continue,
        };
        let df = 链.len() as f64;
        // 非负 IDF 变体：ln(1 + (N - df + 0.5) / (df + 0.5))
        let idf = (1.0 + (n as f64 - df + 0.5) / (df + 0.5)).ln();
        for (槽位, tf) in 链 {
            let 长度 = 索.文档[*槽位 as usize].1 as f64;
            let tf = *tf as f64;
            let 分 = idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * 长度 / 平均长度.max(1e-9)));
            *得分.entry(*槽位).or_insert(0.0) += 分;
        }
    }

    let mut 排名: Vec<(i64, f64)> = 得分
        .into_iter()
        .map(|(槽位, 分)| (索.文档[槽位 as usize].0, 分))
        .collect();
    排名.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    排名.truncate(if k > 0 { k } else { 排名.len() });
    排名
}

fn 取文本(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string()
}

/// 清空/新建 键 对应的词法索引。
#[no_mangle]
pub extern "C" fn qi_lexidx_reset(键: i64) -> i64 {
    let mut 池 = 获取索引池().lock().unwrap();
    池.insert(键, 索引::default());
    1
}

/// 往 键 索引里加一篇 (id, 文本)。空文本忽略。返回索引当前文档数。
#[no_mangle]
pub extern "C" fn qi_lexidx_add(键: i64, id: i64, 文本ptr: *const c_char) -> i64 {
    let 文本 = 取文本(文本ptr);
    let mut 池 = 获取索引池().lock().unwrap();
    let 索 = 池.entry(键).or_default();
    if 文本.is_empty() {
        return 索.文档.len() as i64;
    }
    索引加文档(索, id, &文本) as i64
}

/// 在 键 索引里 BM25 搜 top-K，返回 JSON 数组 [{"id":..,"score":..}]（按分降序）。
/// 无命中/索引不存在 → "[]"。
#[no_mangle]
pub extern "C" fn qi_lexidx_search(键: i64, 查询ptr: *const c_char, k: i64) -> *mut c_char {
    let 查询 = 取文本(查询ptr);
    let 空 = || crate::stdlib::qi_str::rc_cstr_from_string("[]".to_string());
    if 查询.is_empty() {
        return 空();
    }
    let 池 = 获取索引池().lock().unwrap();
    let 索 = match 池.get(&键) {
        Some(s) => s,
        None => return 空(),
    };
    let 排名 = 索引搜索(索, &查询, if k > 0 { k as usize } else { usize::MAX });
    let 结果值: Vec<Value> = 排名
        .iter()
        .map(|(id, 分)| json!({"id": id, "score": 分}))
        .collect();
    crate::stdlib::qi_str::rc_cstr_from_string(Value::Array(结果值).to_string())
}

/// 键 索引文档数。
#[no_mangle]
pub extern "C" fn qi_lexidx_size(键: i64) -> i64 {
    let 池 = 获取索引池().lock().unwrap();
    池.get(&键).map(|s| s.文档.len() as i64).unwrap_or(0)
}

/// 释放 键 索引。
#[no_mangle]
pub extern "C" fn qi_lexidx_free(键: i64) -> i64 {
    let mut 池 = 获取索引池().lock().unwrap();
    池.remove(&键);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 中文bigram切分正确() {
        let 词表 = 切词("混合检索");
        assert_eq!(词表, vec!["混", "合", "混合", "检", "合检", "索", "检索"]);
    }

    #[test]
    fn 标点打断bigram() {
        let 词表 = 切词("你好，世界");
        // 不应出现跨标点的 "好世"
        assert!(!词表.iter().any(|t| t == "好世"));
        assert!(词表.iter().any(|t| t == "你好"));
        assert!(词表.iter().any(|t| t == "世界"));
    }

    #[test]
    fn ascii按标点切词并小写() {
        let 词表 = 切词("Hello, QX-9000 World!");
        assert_eq!(词表, vec!["hello", "qx", "9000", "world"]);
    }

    #[test]
    fn 中英混排各归各() {
        let 词表 = 切词("产品QX-9000量产");
        // ASCII 词与中文单字/bigram 共存，且 ASCII 打断中文 bigram（无 "品量"）
        assert!(词表.contains(&"qx".to_string()));
        assert!(词表.contains(&"9000".to_string()));
        assert!(词表.contains(&"产品".to_string()));
        assert!(词表.contains(&"量产".to_string()));
        assert!(!词表.contains(&"品量".to_string()));
    }

    #[test]
    fn bm25查询词多的文档分高() {
        let mut 索 = 索引::default();
        索引加文档(&mut 索, 1, "混合检索 结合 混合检索 的两路召回"); // 查询词出现两次
        索引加文档(&mut 索, 2, "混合检索 是一种方法，另外还有别的很多方法可以选");
        索引加文档(&mut 索, 3, "今天天气很好，出去散步");
        let 排名 = 索引搜索(&索, "混合检索", 10);
        assert_eq!(排名.len(), 2, "无关文档不应命中: {:?}", 排名);
        assert_eq!(排名[0].0, 1, "查询词出现更多、文档更短者应排前: {:?}", 排名);
        assert_eq!(排名[1].0, 2);
        assert!(排名[0].1 > 排名[1].1);
    }

    #[test]
    fn bm25精确代号必中() {
        let mut 索 = 索引::default();
        索引加文档(&mut 索, 1, "产品代号 QX-9000 已进入量产阶段");
        索引加文档(&mut 索, 2, "把热点数据放进缓存能减少数据库压力");
        let 排名 = 索引搜索(&索, "QX-9000 进度如何", 5);
        assert!(!排名.is_empty());
        assert_eq!(排名[0].0, 1);
    }

    #[test]
    fn 空索引与空查询安全() {
        let 索 = 索引::default();
        assert!(索引搜索(&索, "任意", 5).is_empty());
        let mut 索2 = 索引::default();
        索引加文档(&mut 索2, 1, "内容");
        assert!(索引搜索(&索2, "", 5).is_empty());
    }

    #[test]
    fn ffi往返() {
        use std::ffi::CString;
        let 键 = 987_654_321;
        qi_lexidx_reset(键);
        let 文一 = CString::new("数据库连接池优化吞吐").unwrap();
        let 文二 = CString::new("食堂新增川菜窗口").unwrap();
        assert_eq!(qi_lexidx_add(键, 11, 文一.as_ptr()), 1);
        assert_eq!(qi_lexidx_add(键, 22, 文二.as_ptr()), 2);
        assert_eq!(qi_lexidx_size(键), 2);
        let 查 = CString::new("连接池").unwrap();
        let 回 = qi_lexidx_search(键, 查.as_ptr(), 3);
        let 回文本 = unsafe { CStr::from_ptr(回) }.to_string_lossy().to_string();
        let v: Value = serde_json::from_str(&回文本).unwrap();
        assert_eq!(v[0]["id"].as_i64(), Some(11));
        qi_lexidx_free(键);
        assert_eq!(qi_lexidx_size(键), 0);
    }
}
