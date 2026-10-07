//! Queries as mxrb's `Oql::Translator` reads them: character by character,
//! strings, quoted identifiers and comments kept whole.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Space,
    Text,
    QuotedIdentifier,
    Comment,
    Parameter,
    Word,
    Symbol,
    PathSeparator,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) kind: Kind,
    pub(crate) text: String,
}

impl Token {
    pub(crate) fn new(kind: Kind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

pub(crate) fn tokenize(source: &str) -> Vec<Token> {
    let characters: Vec<char> = source.chars().collect();
    let text = |from: usize, to: usize| characters[from..to].iter().collect::<String>();
    let space = |character: char| matches!(character, ' ' | '\t' | '\r' | '\n' | '\x0b' | '\x0c');
    let word_start = |character: char| character.is_ascii_alphabetic() || character == '_';
    let word_body =
        |character: char| character.is_ascii_alphanumeric() || character == '_' || character == '$';
    let quoted = |start: usize, delimiter: char| {
        let mut finish = start + 1;
        while finish < characters.len() {
            if characters[finish] == delimiter {
                finish += 1;
                if characters.get(finish) == Some(&delimiter) {
                    finish += 1;
                    continue;
                }
                break;
            }
            finish += 1;
        }
        finish
    };
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        let next = characters.get(index + 1).copied();
        let (kind, finish) = if space(character) {
            let mut finish = index;
            while finish < characters.len() && space(characters[finish]) {
                finish += 1;
            }
            (Kind::Space, finish)
        } else if character == '\'' {
            (Kind::Text, quoted(index, '\''))
        } else if character == '"' {
            (Kind::QuotedIdentifier, quoted(index, '"'))
        } else if character == '[' {
            (Kind::QuotedIdentifier, quoted(index, ']'))
        } else if character == '-' && next == Some('-') {
            let finish = (index..characters.len())
                .find(|&at| characters[at] == '\n')
                .unwrap_or(characters.len());
            (Kind::Comment, finish)
        } else if character == '/' && next == Some('*') {
            let finish = (index + 2..characters.len().saturating_sub(1))
                .find(|&at| characters[at] == '*' && characters[at + 1] == '/')
                .map_or(characters.len(), |at| at + 2);
            (Kind::Comment, finish)
        } else if character == '$' && next.is_some_and(word_start) {
            let mut finish = index + 1;
            while finish < characters.len() && word_body(characters[finish]) {
                finish += 1;
            }
            (Kind::Parameter, finish)
        } else if word_start(character) {
            let mut finish = index;
            while finish < characters.len() && word_body(characters[finish]) {
                finish += 1;
            }
            (Kind::Word, finish)
        } else {
            (Kind::Symbol, index + 1)
        };
        tokens.push(Token::new(kind, text(index, finish)));
        index = finish;
    }
    tokens
}
