//! Shell-quoted values, so the config file stays readable by bash. Reading covers everything
//! bash's `printf '%q'` writes (backslash escapes, `'single'`, `"double"` and `$'ansi-c'`
//! quoting, also mixed within one word). Nothing is expanded: a `$NAME`, `$(…)` or backtick that
//! bash would expand is an error instead of being taken literally.
//!
//! Error messages never quote the value, which may be an API key.

/// Parses the shell word at the start of `text`, for example the value after `KEY=`. Returns the
/// value and the rest of the line, which starts at the first unquoted blank.
pub fn parse_word(text: &str) -> Result<(String, &str), String> {
    let bytes = text.as_bytes();
    let mut value = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b' ' | b'\t' => break,
            b'\\' => {
                let Some(&escaped) = bytes.get(index + 1) else {
                    return Err("a backslash ends the line".to_string());
                };
                value.push(escaped);
                index += 2;
            }
            b'\'' => {
                let start = index + 1;
                let length = bytes[start..]
                    .iter()
                    .position(|&b| b == b'\'')
                    .ok_or("unterminated single quote")?;
                value.extend_from_slice(&bytes[start..start + length]);
                index = start + length + 1;
            }
            b'"' => index = parse_double_quoted(bytes, index + 1, &mut value)?,
            b'$' if bytes.get(index + 1) == Some(&b'\'') => index = parse_ansi_c(bytes, index + 2, &mut value)?,
            b'$' if bytes.get(index + 1) == Some(&b'"') || starts_expansion(bytes.get(index + 1)) => {
                return Err("expansions ($…) are not supported".to_string());
            }
            b'`' => return Err("command substitution is not supported".to_string()),
            special @ (b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')') => {
                return Err(format!("unexpected `{}`", char::from(special)));
            }
            other => {
                value.push(other);
                index += 1;
            }
        }
    }
    let value = String::from_utf8(value).map_err(|_| "the value is not valid UTF-8".to_string())?;
    Ok((value, &text[index..]))
}

/// Whether `$` followed by this byte starts a parameter expansion or substitution in bash.
fn starts_expansion(next: Option<&u8>) -> bool {
    next.is_some_and(|&b| b.is_ascii_alphanumeric() || b"_{(@*#?$!-".contains(&b))
}

/// Reads a `"…"` string after its opening quote. Inside it bash only unescapes `\$`, `` \` ``,
/// `\"` and `\\`; any other backslash is kept.
fn parse_double_quoted(bytes: &[u8], mut index: usize, value: &mut Vec<u8>) -> Result<usize, String> {
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Ok(index + 1),
            b'\\' if matches!(bytes.get(index + 1), Some(b'$' | b'`' | b'"' | b'\\')) => {
                value.push(bytes[index + 1]);
                index += 2;
            }
            b'$' if starts_expansion(bytes.get(index + 1)) => {
                return Err("expansions ($…) are not supported".to_string());
            }
            b'`' => return Err("command substitution is not supported".to_string()),
            other => {
                value.push(other);
                index += 1;
            }
        }
    }
    Err("unterminated double quote".to_string())
}

/// Reads a `$'…'` string after its opening quote, with bash's ANSI-C escapes. Octal and `\x`
/// escapes produce bytes, as in bash, so `$'\303\251'` is `é` in UTF-8.
fn parse_ansi_c(bytes: &[u8], mut index: usize, value: &mut Vec<u8>) -> Result<usize, String> {
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte == b'\'' {
            return Ok(index);
        }
        if byte != b'\\' {
            value.push(byte);
            continue;
        }
        let Some(&escape) = bytes.get(index) else { break };
        index += 1;
        match escape {
            b'a' => value.push(0x07),
            b'b' => value.push(0x08),
            b'e' | b'E' => value.push(0x1b),
            b'f' => value.push(0x0c),
            b'n' => value.push(b'\n'),
            b'r' => value.push(b'\r'),
            b't' => value.push(b'\t'),
            b'v' => value.push(0x0b),
            b'\\' | b'\'' | b'"' | b'?' => value.push(escape),
            b'0'..=b'7' => {
                let (code, used) = read_digits(&bytes[index - 1..], 3, 8);
                value.push((code & 0xff) as u8);
                index += used - 1;
            }
            b'x' | b'u' | b'U' => {
                let width = match escape {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                let (code, used) = read_digits(&bytes[index..], width, 16);
                index += used;
                if used == 0 {
                    value.extend_from_slice(&[b'\\', escape]);
                } else if escape == b'x' {
                    value.push(code as u8);
                } else {
                    let c = char::from_u32(code).ok_or("invalid \\u escape")?;
                    value.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                }
            }
            b'c' => {
                let Some(&control) = bytes.get(index) else { break };
                value.push(control & 0x1f);
                index += 1;
            }
            other => value.extend_from_slice(&[b'\\', other]),
        }
    }
    Err("unterminated $'…' quote".to_string())
}

/// Reads up to `max` digits in `radix`; returns the number and how many digits it used.
fn read_digits(bytes: &[u8], max: usize, radix: u32) -> (u32, usize) {
    let mut code = 0u32;
    let mut used = 0;
    for &byte in bytes.iter().take(max) {
        let Some(digit) = char::from(byte).to_digit(radix) else {
            break;
        };
        code = code * radix + digit;
        used += 1;
    }
    (code, used)
}

/// Quotes a single-line value so bash and [`parse_word`] read it back unchanged: as is when it
/// only has characters that are never special, else in single quotes.
pub fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-./:@%+,=".contains(&b));
    if plain {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str) -> Result<String, String> {
        parse_word(text).map(|(value, _)| value)
    }

    #[test]
    fn reads_what_printf_q_writes() {
        // Each pair is a value and what `printf '%q'` prints for it (checked with bash 5.3).
        let cases = [
            ("http://192.0.2.10:8096", "http://192.0.2.10:8096"),
            ("0123456789abcdef", "0123456789abcdef"),
            ("with space", r"with\ space"),
            ("it's", r"it\'s"),
            ("a\"b$c`d\\e", r#"a\"b\$c\`d\\e"#),
            ("~home", r"\~home"),
            ("tab\there", r"$'tab\there'"),
            ("line\nbreak", r"$'line\nbreak'"),
            ("", "''"),
            ("café", "café"),
        ];
        for (value, quoted) in cases {
            assert_eq!(word(quoted).as_deref(), Ok(value), "{quoted}");
        }
    }

    #[test]
    fn reads_single_and_double_quotes_and_mixed_words() {
        assert_eq!(word("'a b \\ \" $x'").as_deref(), Ok("a b \\ \" $x"));
        assert_eq!(word(r#""a \"b\" \$c \\ \d""#).as_deref(), Ok(r#"a "b" $c \ \d"#));
        assert_eq!(word(r"'it'\''s'").as_deref(), Ok("it's"));
        assert_eq!(word(r#"pre'mid'"post"$'\x41'"#).as_deref(), Ok("premidpostA"));
        assert_eq!(word(r#""cost: $ 5""#).as_deref(), Ok("cost: $ 5"));
    }

    #[test]
    fn reads_ansi_c_escapes() {
        assert_eq!(
            word(r"$'\a\b\e\E\f\n\r\t\v'").as_deref(),
            Ok("\x07\x08\x1b\x1b\x0c\n\r\t\x0b")
        );
        assert_eq!(word(r#"$'\\ \' \" \?'"#).as_deref(), Ok("\\ ' \" ?"));
        assert_eq!(word(r"$'\101\0\7'").as_deref(), Ok("A\0\x07"));
        assert_eq!(word(r"$'\303\251'").as_deref(), Ok("é"));
        assert_eq!(word(r"$'\x41\x4a\xz'").as_deref(), Ok("AJ\\xz"));
        assert_eq!(word(r"$'é\U0001F3B5'").as_deref(), Ok("é🎵"));
        assert_eq!(word(r"$'\cA\q'").as_deref(), Ok("\x01\\q"));
    }

    #[test]
    fn stops_at_the_first_unquoted_blank() {
        assert_eq!(parse_word("value # comment"), Ok(("value".to_string(), " # comment")));
        assert_eq!(parse_word("'a b'\tc"), Ok(("a b".to_string(), "\tc")));
    }

    #[test]
    fn rejects_what_bash_would_expand_or_cannot_read() {
        for text in [
            "$HOME",
            "a${x}",
            "$(id)",
            "`id`",
            "\"$USER\"",
            "\"`id`\"",
            "a;b",
            "a|b",
            "'open",
            "\"open",
            "$'open",
            "end\\",
            "$\"x\"",
            r"$'\xff'",
        ] {
            assert!(word(text).is_err(), "{text}");
        }
    }

    #[test]
    fn errors_do_not_contain_the_value() {
        let secret = "s3cr3t-api-key";
        for text in [format!("'{secret}"), format!("{secret}$HOME"), format!("\"{secret}")] {
            let error = word(&text).expect_err("invalid");
            assert!(!error.contains(secret), "{error}");
        }
    }

    #[test]
    fn quotes_only_when_needed_and_round_trips() {
        assert_eq!(quote("http://192.0.2.10:8096"), "http://192.0.2.10:8096");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("~x"), "'~x'");
        for value in [
            "plain",
            "with space",
            "it's 'quoted'",
            "$HOME `x` \"y\" \\z",
            "",
            "~",
            "#hash",
        ] {
            assert_eq!(word(&quote(value)).as_deref(), Ok(value));
        }
    }
}
