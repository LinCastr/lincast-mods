//! Leitor mínimo do formato KeyValues em texto (VDF) do Steam:
//! `libraryfolders.vdf`, `appmanifest_*.acf`, `config.vdf`.

#[derive(Debug, Clone)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    /// Filho pelo nome, sem diferenciar maiúsculas (o Steam mistura "AppState"/"appstate").
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(items) => items
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(items) => items,
            Vdf::Str(_) => &[],
        }
    }
}

enum Tok {
    Str(String),
    Open,
    Close,
}

fn tokenize(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => break,
                        },
                        _ => s.push(c),
                    }
                }
                out.push(Tok::Str(s));
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            // Token sem aspas (raro, mas existe em arquivos antigos).
            _ => {
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '{' || n == '}' || n == '"' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                out.push(Tok::Str(s));
            }
        }
    }
    out
}

fn parse_obj(toks: &mut std::iter::Peekable<std::vec::IntoIter<Tok>>) -> Vec<(String, Vdf)> {
    let mut items = Vec::new();
    while let Some(tok) = toks.next() {
        match tok {
            Tok::Close => break,
            Tok::Open => {}
            Tok::Str(key) => match toks.next() {
                Some(Tok::Str(val)) => items.push((key, Vdf::Str(val))),
                Some(Tok::Open) => items.push((key, Vdf::Obj(parse_obj(toks)))),
                _ => break,
            },
        }
    }
    items
}

pub fn parse(src: &str) -> Vdf {
    let mut toks = tokenize(src).into_iter().peekable();
    Vdf::Obj(parse_obj(&mut toks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_libraryfolders() {
        let v = parse(
            r#""libraryfolders" { "0" { "path" "/home/x/Steam" "apps" { "413150" "123" } } // fim
               "1" { "path" "/mnt/Jogos\\SteamLibrary" } }"#,
        );
        let lf = v.get("LibraryFolders").unwrap();
        assert_eq!(lf.entries().len(), 2);
        assert_eq!(lf.get("0").unwrap().str("path"), Some("/home/x/Steam"));
        assert_eq!(lf.get("1").unwrap().str("path"), Some("/mnt/Jogos\\SteamLibrary"));
    }
}
