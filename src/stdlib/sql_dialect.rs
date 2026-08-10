//! SQL 方言：把 `?` 占位符改写成 PostgreSQL 的 `$1 $2 …`
//!
//! ── 为什么由中间层翻译，而不是让应用改写 SQL ─────────────────────
//!
//! `qi-web/docs/多数据库设计.md` 4.1：现有 12+ 个 qi 应用里 45 处 SQL 全部用 `?`
//! （`VALUES (?, ?, ?)`、`WHERE 用户id = ?`）。SQLite 和 MySQL 认 `?`，PostgreSQL
//! 只认 `$n`。JDBC / PDO 的做法是对外永远 `?`、由驱动翻译；Go `database/sql` 和
//! Python DB-API 把方言暴露给用户（`$1` vs `?`、`paramstyle`），公认是败笔。
//! 这里跟 JDBC 走，于是那 45 处一行都不用改。
//!
//! ── 为什么不能无脑 replace ───────────────────────────────────────
//!
//! `?` 可能出现在字符串字面量、双引号标识符、注释、美元引用串里，那些位置的 `?`
//! 改掉就是静默改坏 SQL（最坏情况是把用户数据里的问号变成参数）。所以逐字节扫描，
//! 维护「普通 / 单引号串 / 定界标识符 / 行注释 / 块注释 / 美元引用」状态，
//! 只在普通状态下替换。
//!
//! ── jsonb 的 `?` `?|` `?&` 运算符：业界怎么解决的 ─────────────────
//!
//! PG 的 jsonb「键是否存在」运算符恰好也写作 `?`，和占位符字面上无法区分
//! （`WHERE 配置 ? 'k'` vs `WHERE 名 = ?`）。调研结论：
//!
//! * **pgjdbc** —— 不猜。`?` 一律当占位符，要写运算符就双写成 `??`，驱动再还原成
//!   单个 `?`（pgjdbc/pgjdbc#643）。
//! * **PHP PDO_PGSQL** —— 后来抄了 pgjdbc 这套 `??` 转义。
//! * **Perl DBD::Pg** —— 走的是 `\?` 转义（另一套，未成主流）。
//! * **ODBC** —— 干脆不支持，ODBC 规范里 `?` 就是参数。
//! * **tokio-postgres / sqlx / pgx** —— 根本不做翻译，原生就写 `$n`，绕开了整个问题。
//! * **官方出路** —— 用函数形式 `jsonb_exists(列,'k')` / `jsonb_exists_any` /
//!   `jsonb_exists_all` 代替运算符，ORM 场景推荐这条。
//!
//! 本实现取 pgjdbc/PDO 的多数派方案，判定规则如下：
//!
//! 1. `??` → 一个字面 `?`（jsonb exists 运算符的转义），`??|` `??&` 同理。
//!    只支持这一种转义：再加 `\?` 会让 `\??` 变得没法解释。
//! 2. 裸的 `?|` / `?&` → **报错**。这两个在 PG 里是不可拆的单个运算符 token，
//!    但改成 `$n|` `$n&` 又能被 PG 解析成「参数 位或/位与 某值」，属于会静默算错的
//!    那一类，宁可报错也不猜（设计文档风险表的要求）。错误信息里给出两条出路。
//! 3. `@?`（jsonpath exists）原样保留、不占参数序号：`@` 后面在三种方言里都不可能
//!    是一个值的位置，不存在歧义。
//! 4. 其余裸 `?` 一律当占位符。**猜错也不会静默算错数据**：jsonb 的 `?` 被改成
//!    `列 $1 'k'` 在 PG 里是语法错误，会当场报错；反过来漏改的 `?` 在 PG 里同样是
//!    语法错误。两个方向都是响亮的失败，这正是敢定这条默认规则的原因。
//! 5. 整条 SQL 里一个 `?` 都没有 → 原样返回，一个字节都不动（PG 原生 `$n` SQL
//!    经过本层必须完全无损）。
//! 6. `$n` 与 `?` 混用 → 报错。序号会撞车，与其编号错乱不如让人改 SQL。

// 函数名里嵌了 ASCII 的 PG，rustc 会以为是驼峰
#![allow(non_snake_case)]

/// 标识符里可以出现的字节。用来判断 `E'…'` 里的 E 是转义串前缀，还是某个名字的末字符
/// （`价格E'x'` 不存在，但 `表E` 后面紧跟串的写法要防）。
fn 是标识符字节(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// 把 `?` 占位符改写成 PostgreSQL 的 `$1 $2 …`
///
/// 返回改写后的 SQL；遇到不该改的位置原样保留。无法可靠判定时返回 `Err`。
pub fn 改写为PG占位符(sql: &str) -> Result<String, String> {
    改写为PG占位符并计数(sql).map(|(改写后, _)| 改写后)
}

/// 同 [`改写为PG占位符`]，另外给出本次改写产生的占位符个数。
///
/// 驱动层拿它校验实参个数 —— 传少了直接读到垃圾寄存器这种事在本仓已经出过一次。
/// 注意：SQL 本来就是 `$n` 写法（不含 `?`）时个数为 0，因为那种 SQL 不经过改写。
pub fn 改写为PG占位符并计数(sql: &str) -> Result<(String, usize), String> {
    let 字节 = sql.as_bytes();
    if !字节.contains(&b'?') {
        return Ok((sql.to_string(), 0));
    }

    let 长 = 字节.len();
    let mut 出: Vec<u8> = Vec::with_capacity(长 + 8);
    let mut i = 0usize;
    let mut 序号 = 0usize;
    let mut 有美元占位符 = false;

    while i < 长 {
        let c = 字节[i];
        match c {
            b'\'' => i = 抄单引号串(字节, i, &mut 出, false)?,

            // E'…' / e'…'：PG 里只有这种串认反斜杠转义，普通 '…' 不认
            // （standard_conforming_strings 默认 on）。区分开才能正确判断串在哪结束。
            b'E' | b'e'
                if i + 1 < 长
                    && 字节[i + 1] == b'\''
                    && (i == 0 || !是标识符字节(字节[i - 1])) =>
            {
                出.push(c);
                i = 抄单引号串(字节, i + 1, &mut 出, true)?;
            }

            b'"' => i = 抄定界标识符(字节, i, &mut 出, b'"')?,

            // 反引号是 MySQL 的标识符定界符。这层输出是给 PG 的、按说不该出现，
            // 但同一条 SQL 常量在应用里可能两个后端共用，跳过它有益无害。
            b'`' => i = 抄定界标识符(字节, i, &mut 出, b'`')?,

            b'-' if i + 1 < 长 && 字节[i + 1] == b'-' => i = 抄行注释(字节, i, &mut 出),

            b'/' if i + 1 < 长 && 字节[i + 1] == b'*' => i = 抄块注释(字节, i, &mut 出)?,

            b'$' => {
                if i + 1 < 长 && 字节[i + 1].is_ascii_digit() {
                    有美元占位符 = true;
                    出.push(b'$');
                    i += 1;
                    while i < 长 && 字节[i].is_ascii_digit() {
                        出.push(字节[i]);
                        i += 1;
                    }
                } else if let Some(标签末) = 美元引用开头(字节, i) {
                    i = 抄美元引用串(字节, i, 标签末, &mut 出)?;
                } else {
                    出.push(b'$');
                    i += 1;
                }
            }

            // jsonpath exists：`数据 @? '$.a'`
            b'@' if i + 1 < 长 && 字节[i + 1] == b'?' => {
                出.extend_from_slice(b"@?");
                i += 2;
            }

            b'?' => match 字节.get(i + 1).copied() {
                Some(b'?') => {
                    // pgjdbc/PDO 的转义：?? 还原成一个字面 ?（jsonb exists 运算符）
                    出.push(b'?');
                    i += 2;
                }
                Some(b'|') | Some(b'&') => {
                    return Err(format!(
                        "SQL 里出现裸的 `?{}`，无法判断是 jsonb 运算符还是占位符：\
                         写成 `??{}` 转义，或改用 jsonb_exists_any / jsonb_exists_all 函数",
                        字节[i + 1] as char,
                        字节[i + 1] as char
                    ));
                }
                _ => {
                    序号 += 1;
                    出.push(b'$');
                    出.extend_from_slice(序号.to_string().as_bytes());
                    i += 1;
                }
            },

            _ => {
                出.push(c);
                i += 1;
            }
        }
    }

    if 有美元占位符 && 序号 > 0 {
        return Err("同一条 SQL 里混用了 $n 与 ? 两种占位符，序号会撞车：请统一成 ?".to_string());
    }

    // 只在 ASCII 边界上切割和插入，多字节字符不会被劈开
    let 改写后 = String::from_utf8(出).map_err(|_| "改写结果不是合法 UTF-8".to_string())?;
    Ok((改写后, 序号))
}

/// 抄一段 `'…'`。`起` 指向开头的单引号，返回结束引号之后的下标。
///
/// `双写转义` 是 SQL 标准的 `''`，任何方言都得认；`反斜杠转义` 只对 `E'…'` 开。
fn 抄单引号串(
    字节: &[u8],
    起: usize,
    出: &mut Vec<u8>,
    反斜杠转义: bool,
) -> Result<usize, String> {
    let 长 = 字节.len();
    出.push(b'\'');
    let mut j = 起 + 1;
    while j < 长 {
        let c = 字节[j];
        if 反斜杠转义 && c == b'\\' && j + 1 < 长 {
            出.push(c);
            出.push(字节[j + 1]);
            j += 2;
            continue;
        }
        if c == b'\'' {
            if j + 1 < 长 && 字节[j + 1] == b'\'' {
                出.push(b'\'');
                出.push(b'\'');
                j += 2;
                continue;
            }
            出.push(b'\'');
            return Ok(j + 1);
        }
        出.push(c);
        j += 1;
    }
    Err("SQL 里有未闭合的字符串字面量（'）".to_string())
}

/// 抄一段定界标识符：`"…"`（SQL 标准 / PG）或 `` `…` ``（MySQL）。两者都用「双写」转义定界符。
fn 抄定界标识符(
    字节: &[u8], 起: usize, 出: &mut Vec<u8>, 界: u8
) -> Result<usize, String> {
    let 长 = 字节.len();
    出.push(界);
    let mut j = 起 + 1;
    while j < 长 {
        let c = 字节[j];
        if c == 界 {
            if j + 1 < 长 && 字节[j + 1] == 界 {
                出.push(界);
                出.push(界);
                j += 2;
                continue;
            }
            出.push(界);
            return Ok(j + 1);
        }
        出.push(c);
        j += 1;
    }
    Err(format!("SQL 里有未闭合的定界标识符（{}）", 界 as char))
}

/// 抄 `-- …` 到行尾。换行本身留给主循环，省得处理 `\r\n`。
fn 抄行注释(字节: &[u8], 起: usize, 出: &mut Vec<u8>) -> usize {
    let mut j = 起;
    while j < 字节.len() && 字节[j] != b'\n' {
        出.push(字节[j]);
        j += 1;
    }
    j
}

/// 抄 `/* … */`。PG 的块注释**可以嵌套**（和 C 不同），所以要数深度，
/// 否则 `/* a /* ? */ b */` 会在第一个 `*/` 就以为注释结束，把后面的 `?` 当占位符改掉。
fn 抄块注释(字节: &[u8], 起: usize, 出: &mut Vec<u8>) -> Result<usize, String> {
    let 长 = 字节.len();
    let mut 深 = 0usize;
    let mut j = 起;
    while j < 长 {
        if 字节[j] == b'/' && j + 1 < 长 && 字节[j + 1] == b'*' {
            深 += 1;
            出.extend_from_slice(b"/*");
            j += 2;
            continue;
        }
        if 字节[j] == b'*' && j + 1 < 长 && 字节[j + 1] == b'/' {
            深 -= 1;
            出.extend_from_slice(b"*/");
            j += 2;
            if 深 == 0 {
                return Ok(j);
            }
            continue;
        }
        出.push(字节[j]);
        j += 1;
    }
    Err("SQL 里有未闭合的块注释（/*）".to_string())
}

/// `起` 指向 `$`。若这里是美元引用串的开定界符（`$$` 或 `$标签$`），返回定界符之后的下标。
///
/// 标签首字符不能是数字 —— 正是这一条把 `$1` 挡在外面，让它继续被当作 PG 占位符。
fn 美元引用开头(字节: &[u8], 起: usize) -> Option<usize> {
    let 长 = 字节.len();
    let mut j = 起 + 1;
    if j < 长 && 字节[j] == b'$' {
        return Some(j + 1);
    }
    if j >= 长 || 字节[j].is_ascii_digit() || !是标识符字节(字节[j]) {
        return None;
    }
    while j < 长 && 是标识符字节(字节[j]) {
        j += 1;
    }
    if j < 长 && 字节[j] == b'$' {
        Some(j + 1)
    } else {
        None
    }
}

/// 抄 `$标签$ … $标签$`。内容一字不动 —— 里面通常是 PL/pgSQL 函数体，
/// 什么字符都可能有。
fn 抄美元引用串(
    字节: &[u8],
    起: usize,
    标签末: usize,
    出: &mut Vec<u8>,
) -> Result<usize, String> {
    let 定界 = &字节[起..标签末];
    let mut j = 标签末;
    while j + 定界.len() <= 字节.len() {
        if &字节[j..j + 定界.len()] == 定界 {
            let 末 = j + 定界.len();
            出.extend_from_slice(&字节[起..末]);
            return Ok(末);
        }
        j += 1;
    }
    Err(format!(
        "SQL 里有未闭合的美元引用字符串（{}）",
        String::from_utf8_lossy(定界)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 改(sql: &str) -> String {
        改写为PG占位符(sql).unwrap()
    }

    // ── 基本 ──────────────────────────────────────────────

    #[test]
    fn 空串() {
        assert_eq!(改(""), "");
    }

    #[test]
    fn 无占位符原样返回() {
        let sql = "SELECT 1 FROM 用户 WHERE 启用 = TRUE";
        assert_eq!(改(sql), sql);
    }

    #[test]
    fn 已是美元占位符的原样返回() {
        let sql = "SELECT * FROM 卡片 WHERE 用户id = $1 AND 状态 = $2";
        assert_eq!(改(sql), sql);
        // 不含 ? 时不产生新占位符
        assert_eq!(改写为PG占位符并计数(sql).unwrap().1, 0);
    }

    #[test]
    fn 只有一个问号() {
        assert_eq!(改("?"), "$1");
    }

    #[test]
    fn 问号在末尾() {
        assert_eq!(
            改("SELECT * FROM 卡片 WHERE id = ?"),
            "SELECT * FROM 卡片 WHERE id = $1"
        );
    }

    #[test]
    fn 多参数按序编号() {
        assert_eq!(
            改("INSERT INTO 卡片 (用户id, 标题, 内容) VALUES (?, ?, ?)"),
            "INSERT INTO 卡片 (用户id, 标题, 内容) VALUES ($1, $2, $3)"
        );
    }

    #[test]
    fn 紧贴运算符没有空格也认() {
        assert_eq!(改("WHERE a=? AND b=?"), "WHERE a=$1 AND b=$2");
    }

    #[test]
    fn 计数与改写一致() {
        let (改写后, 个数) = 改写为PG占位符并计数("UPDATE t SET a=?, b=? WHERE id=?").unwrap();
        assert_eq!(改写后, "UPDATE t SET a=$1, b=$2 WHERE id=$3");
        assert_eq!(个数, 3);
    }

    // ── 字符串字面量 ──────────────────────────────────────

    #[test]
    fn 单引号串里的问号不动() {
        assert_eq!(
            改("SELECT * FROM t WHERE 备注 = '真的吗？really?' AND id = ?"),
            "SELECT * FROM t WHERE 备注 = '真的吗？really?' AND id = $1"
        );
    }

    #[test]
    fn 单引号双写转义() {
        // 'it''s ?' 整体是一个串，里面的 ? 不能改
        assert_eq!(改("SELECT 'it''s ?', ?"), "SELECT 'it''s ?', $1");
    }

    #[test]
    fn 连续两个串各自闭合() {
        assert_eq!(改("SELECT 'a?' , 'b?' , ?"), "SELECT 'a?' , 'b?' , $1");
    }

    #[test]
    fn E串认反斜杠转义() {
        // E'it\'s ?' 里 \' 不结束串，? 仍在串内
        assert_eq!(改(r"SELECT E'it\'s ?', ?"), r"SELECT E'it\'s ?', $1");
    }

    #[test]
    fn 普通串不认反斜杠转义() {
        // standard_conforming_strings=on：'a\' 到这里就结束了，后面的 ? 是占位符
        assert_eq!(改(r"SELECT 'a\', ?"), r"SELECT 'a\', $1");
    }

    #[test]
    fn 标识符尾巴上的E不算转义串前缀() {
        // 列名 someE 后面紧跟一个普通串，别把 E 吃成前缀
        assert_eq!(改(r"SELECT someE'a\', ?"), r"SELECT someE'a\', $1");
    }

    #[test]
    fn 中文串与中文列名都不被劈开() {
        assert_eq!(
            改("SELECT 标题 FROM 绘本 WHERE 作者 = '张三？' AND 页数 > ?"),
            "SELECT 标题 FROM 绘本 WHERE 作者 = '张三？' AND 页数 > $1"
        );
    }

    // ── 定界标识符 ────────────────────────────────────────

    #[test]
    fn 双引号标识符里的问号不动() {
        assert_eq!(
            改(r#"SELECT "my?col" FROM t WHERE id = ?"#),
            r#"SELECT "my?col" FROM t WHERE id = $1"#
        );
    }

    #[test]
    fn 双引号双写转义() {
        assert_eq!(改(r#"SELECT "a""?b" , ?"#), r#"SELECT "a""?b" , $1"#);
    }

    #[test]
    fn 反引号标识符里的问号不动() {
        assert_eq!(
            改("SELECT `my?col` FROM t WHERE id = ?"),
            "SELECT `my?col` FROM t WHERE id = $1"
        );
    }

    // ── 注释 ──────────────────────────────────────────────

    #[test]
    fn 行注释里的问号不动() {
        assert_eq!(
            改("SELECT 1 -- 这里有个 ? 不算\nWHERE id = ?"),
            "SELECT 1 -- 这里有个 ? 不算\nWHERE id = $1"
        );
    }

    #[test]
    fn 行注释在文件末尾没有换行() {
        assert_eq!(改("SELECT ? -- 尾注释 ?"), "SELECT $1 -- 尾注释 ?");
    }

    #[test]
    fn 块注释里的问号不动() {
        assert_eq!(改("SELECT /* ? 注释 */ ?"), "SELECT /* ? 注释 */ $1");
    }

    #[test]
    fn 嵌套块注释() {
        assert_eq!(
            改("SELECT /* 外 /* 内 ? */ 还在注释里 ? */ ?"),
            "SELECT /* 外 /* 内 ? */ 还在注释里 ? */ $1"
        );
    }

    #[test]
    fn 除号不会被当成块注释() {
        assert_eq!(
            改("SELECT a / b WHERE id = ?"),
            "SELECT a / b WHERE id = $1"
        );
    }

    #[test]
    fn 减号不会被当成行注释() {
        assert_eq!(
            改("SELECT a - b WHERE id = ?"),
            "SELECT a - b WHERE id = $1"
        );
    }

    // ── 美元引用串 ────────────────────────────────────────

    #[test]
    fn 无标签美元引用() {
        assert_eq!(
            改("SELECT $$里面 ? 不改$$, ?"),
            "SELECT $$里面 ? 不改$$, $1"
        );
    }

    #[test]
    fn 带标签美元引用可内含双美元() {
        assert_eq!(
            改("DO $体$ BEGIN 随便 ? $$ 也不改 END $体$; SELECT ?"),
            "DO $体$ BEGIN 随便 ? $$ 也不改 END $体$; SELECT $1"
        );
    }

    #[test]
    fn 孤立美元号原样保留() {
        assert_eq!(改("SELECT '¥' || $ , ?"), "SELECT '¥' || $ , $1");
    }

    // ── jsonb 运算符 ──────────────────────────────────────

    #[test]
    fn 双问号转义成字面问号() {
        assert_eq!(
            改("SELECT * FROM t WHERE 配置 ?? 'key' AND id = ?"),
            "SELECT * FROM t WHERE 配置 ? 'key' AND id = $1"
        );
    }

    #[test]
    fn 双问号竖线与与号() {
        assert_eq!(
            改("WHERE 配置 ??| ARRAY['a'] AND id = ?"),
            "WHERE 配置 ?| ARRAY['a'] AND id = $1"
        );
        assert_eq!(
            改("WHERE 配置 ??& ARRAY['a'] AND id = ?"),
            "WHERE 配置 ?& ARRAY['a'] AND id = $1"
        );
    }

    #[test]
    fn 裸的问号竖线报错() {
        let 错 = 改写为PG占位符("WHERE 配置 ?| ARRAY['a']").unwrap_err();
        assert!(错.contains("jsonb"), "{}", 错);
        assert!(错.contains("??|"), "{}", 错);
    }

    #[test]
    fn 裸的问号与号报错() {
        assert!(改写为PG占位符("WHERE 配置 ?& ARRAY['a']").is_err());
    }

    #[test]
    fn jsonpath存在运算符原样且不占序号() {
        assert_eq!(
            改("SELECT * FROM t WHERE 数据 @? '$.a' AND id = ?"),
            "SELECT * FROM t WHERE 数据 @? '$.a' AND id = $1"
        );
    }

    #[test]
    fn 三个问号是转义加占位符() {
        // ?? 先吃掉两个还原成字面 ?，剩下一个是占位符
        assert_eq!(改("???"), "?$1");
    }

    #[test]
    fn 连续两个问号不会变成两个占位符() {
        // 两个相邻占位符在任何方言里都得有分隔符，所以 ?? 只可能是转义
        assert_eq!(改("??"), "?");
        assert_eq!(改("?,?"), "$1,$2");
    }

    // ── 混用与错误 ────────────────────────────────────────

    #[test]
    fn 混用美元占位符与问号报错() {
        let 错 = 改写为PG占位符("WHERE a = $1 AND b = ?").unwrap_err();
        assert!(错.contains("混用"), "{}", 错);
    }

    #[test]
    fn 未闭合字符串报错() {
        assert!(改写为PG占位符("SELECT 'abc WHERE id = ?").is_err());
    }

    #[test]
    fn 未闭合双引号标识符报错() {
        assert!(改写为PG占位符(r#"SELECT "abc WHERE id = ?"#).is_err());
    }

    #[test]
    fn 未闭合块注释报错() {
        assert!(改写为PG占位符("SELECT /* 注释 WHERE id = ?").is_err());
    }

    #[test]
    fn 未闭合美元引用报错() {
        assert!(改写为PG占位符("DO $体$ BEGIN ? END; SELECT ?").is_err());
    }

    // ── 类型转换与综合 ────────────────────────────────────

    #[test]
    fn 类型转换双冒号不受影响() {
        assert_eq!(
            改("SELECT a::text FROM t WHERE b::int = ? AND c = ?::text"),
            "SELECT a::text FROM t WHERE b::int = $1 AND c = $2::text"
        );
    }

    #[test]
    fn 综合场景() {
        let 原 = "-- 取用户卡片 ?\n\
                  SELECT c.\"标?题\", c.数据 @? '$.封面', 'it''s ? ok' AS 备注\n\
                  FROM 卡片 c /* 注释 /* 嵌套 ? */ 仍在注释 */\n\
                  WHERE c.用户id = ? AND c.配置 ?? 'vip' AND c.标签 = ANY(?)\n\
                  ORDER BY c.创建时间 DESC LIMIT ? OFFSET ?";
        let 期望 = "-- 取用户卡片 ?\n\
                    SELECT c.\"标?题\", c.数据 @? '$.封面', 'it''s ? ok' AS 备注\n\
                    FROM 卡片 c /* 注释 /* 嵌套 ? */ 仍在注释 */\n\
                    WHERE c.用户id = $1 AND c.配置 ? 'vip' AND c.标签 = ANY($2)\n\
                    ORDER BY c.创建时间 DESC LIMIT $3 OFFSET $4";
        let (改写后, 个数) = 改写为PG占位符并计数(原).unwrap();
        assert_eq!(改写后, 期望);
        assert_eq!(个数, 4);
    }

    #[test]
    fn 现有应用里的真实语句() {
        // 取自 项目/ 下应用的常见写法
        assert_eq!(
            改("INSERT INTO 会话 (会话id, 用户id, 过期时间) VALUES (?, ?, ?) ON CONFLICT (会话id) DO UPDATE SET 过期时间 = ?"),
            "INSERT INTO 会话 (会话id, 用户id, 过期时间) VALUES ($1, $2, $3) ON CONFLICT (会话id) DO UPDATE SET 过期时间 = $4"
        );
        assert_eq!(
            改("DELETE FROM 会话 WHERE 用户id = ?"),
            "DELETE FROM 会话 WHERE 用户id = $1"
        );
        assert_eq!(
            改("SELECT * FROM 动作 WHERE 部位 LIKE ? ORDER BY id LIMIT ?"),
            "SELECT * FROM 动作 WHERE 部位 LIKE $1 ORDER BY id LIMIT $2"
        );
    }

    #[test]
    fn 超过九个参数编号正确() {
        let 改写后 = 改(&"?,".repeat(12));
        assert!(改写后.starts_with("$1,$2,"), "{}", 改写后);
        assert!(改写后.contains("$10,$11,$12,"), "{}", 改写后);
    }
}
