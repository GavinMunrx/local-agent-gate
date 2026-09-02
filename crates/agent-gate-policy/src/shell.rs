//! A small shell parser: just enough structure to tell a command from its data.
//!
//! The classifier used to regex the raw command string, which cannot tell
//! `git reset --hard` (a command) from `echo "git reset --hard"` (an argument
//! that happens to quote one). That produced constant false positives on any
//! work that writes about shell commands, and it made every compound command
//! look riskier than its parts.
//!
//! This is deliberately not a complete shell grammar. It resolves word
//! boundaries, quoting, heredocs, redirection, pipelines and command
//! substitution - the structure risk rules actually need - and ignores the
//! rest (parameter expansion, arithmetic, process substitution, control flow).

/// One simple command: a program name and its arguments.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Command {
    pub argv: Vec<String>,
    /// Whether the command redirects output into a file.
    pub writes_file: bool,
}

impl Command {
    /// The program being run, with any leading path stripped.
    pub fn name(&self) -> Option<&str> {
        self.argv
            .first()
            .map(|w| w.rsplit('/').next().unwrap_or(w.as_str()))
    }

    pub fn args(&self) -> &[String] {
        self.argv.get(1..).unwrap_or(&[])
    }

    /// Whether any argument equals one of `flags`, including inside a bundled
    /// short-flag cluster such as `-rf`.
    pub fn has_flag(&self, flags: &[&str]) -> bool {
        self.args().iter().any(|arg| {
            if flags.contains(&arg.as_str()) {
                return true;
            }
            // A bundled cluster like `-rf`, but not a long flag like `--force`.
            if arg.starts_with('-') && !arg.starts_with("--") {
                return flags.iter().any(|f| {
                    f.len() == 2
                        && f.starts_with('-')
                        && arg[1..].contains(f.chars().nth(1).unwrap_or(' '))
                });
            }
            false
        })
    }

    /// Arguments that are not flags.
    pub fn operands(&self) -> Vec<&String> {
        self.args().iter().filter(|a| !a.starts_with('-')).collect()
    }

    /// The first non-flag argument, which is a subcommand for tools like git.
    pub fn subcommand(&self) -> Option<&str> {
        self.operands().first().map(|s| s.as_str())
    }
}

/// One or more commands joined by `|`. Pipelines matter as a unit because
/// data flow across them is itself a risk signal (a secret read piped into a
/// network tool is worse than either half alone).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pipeline {
    pub commands: Vec<Command>,
    /// Whether this came from a `$(...)` or backtick substitution, whose
    /// output is captured into the surrounding command rather than shown.
    pub from_substitution: bool,
}

/// Splits a raw command string into pipelines.
pub fn parse(raw: &str) -> Vec<Pipeline> {
    let mut parser = Parser::new(raw);
    parser.run(false);
    parser.pipelines
}

struct Parser {
    chars: Vec<char>,
    i: usize,
    word: String,
    word_started: bool,
    command: Command,
    pipeline: Pipeline,
    pipelines: Vec<Pipeline>,
    /// Substitution bodies found while scanning, parsed once the pass ends so
    /// their pipelines land after the command that contained them.
    deferred: Vec<String>,
    from_substitution: bool,
}

impl Parser {
    fn new(raw: &str) -> Self {
        Parser {
            chars: raw.chars().collect(),
            i: 0,
            word: String::new(),
            word_started: false,
            command: Command::default(),
            pipeline: Pipeline::default(),
            pipelines: Vec::new(),
            deferred: Vec::new(),
            from_substitution: false,
        }
    }

    fn run(&mut self, from_substitution: bool) {
        self.from_substitution = from_substitution;
        while self.i < self.chars.len() {
            let c = self.chars[self.i];
            match c {
                '\\' => {
                    if self.i + 1 < self.chars.len() {
                        self.push(self.chars[self.i + 1]);
                        self.i += 2;
                    } else {
                        self.i += 1;
                    }
                }
                '\'' => self.read_single_quoted(),
                '"' => self.read_double_quoted(),
                '`' => self.read_backtick(),
                '$' if self.peek(1) == Some('(') => self.read_substitution(),
                '|' => {
                    // `||` ends the pipeline; a single `|` continues it.
                    if self.peek(1) == Some('|') {
                        self.i += 2;
                        self.end_pipeline();
                    } else {
                        self.i += 1;
                        self.end_command();
                    }
                }
                '&' => {
                    self.i += if self.peek(1) == Some('&') { 2 } else { 1 };
                    self.end_pipeline();
                }
                ';' | '\n' => {
                    self.i += 1;
                    self.end_pipeline();
                }
                '(' | ')' | '{' | '}' => {
                    self.i += 1;
                    self.end_pipeline();
                }
                '<' => self.read_input_redirect(),
                '>' => self.read_output_redirect(),
                c if c.is_whitespace() => {
                    self.i += 1;
                    self.end_word();
                }
                c => {
                    self.push(c);
                    self.i += 1;
                }
            }
        }
        self.end_pipeline();

        let deferred = std::mem::take(&mut self.deferred);
        for body in deferred {
            let mut sub = Parser::new(&body);
            sub.run(true);
            self.pipelines.append(&mut sub.pipelines);
        }
    }

    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.i + ahead).copied()
    }

    fn push(&mut self, c: char) {
        self.word.push(c);
        self.word_started = true;
    }

    fn end_word(&mut self) {
        if self.word_started {
            self.command.argv.push(std::mem::take(&mut self.word));
            self.word_started = false;
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        if !self.command.argv.is_empty() || self.command.writes_file {
            let command = std::mem::take(&mut self.command);
            self.pipeline.commands.push(command);
        }
    }

    fn end_pipeline(&mut self) {
        self.end_command();
        if !self.pipeline.commands.is_empty() {
            let mut pipeline = std::mem::take(&mut self.pipeline);
            pipeline.from_substitution = self.from_substitution;
            self.pipelines.push(pipeline);
        }
    }

    fn read_single_quoted(&mut self) {
        self.i += 1;
        self.word_started = true;
        while self.i < self.chars.len() && self.chars[self.i] != '\'' {
            self.word.push(self.chars[self.i]);
            self.i += 1;
        }
        self.i += 1;
    }

    /// Double quotes keep their content as one word, but still expand
    /// substitutions inside - `"$(cat secret)"` is a real command.
    fn read_double_quoted(&mut self) {
        self.i += 1;
        self.word_started = true;
        while self.i < self.chars.len() && self.chars[self.i] != '"' {
            match self.chars[self.i] {
                '\\' if self.i + 1 < self.chars.len() => {
                    self.word.push(self.chars[self.i + 1]);
                    self.i += 2;
                }
                '`' => self.read_backtick(),
                '$' if self.peek(1) == Some('(') => self.read_substitution(),
                c => {
                    self.word.push(c);
                    self.i += 1;
                }
            }
        }
        self.i += 1;
    }

    fn read_substitution(&mut self) {
        // Positioned at `$(`.
        self.i += 2;
        let start = self.i;
        let mut depth = 1;
        while self.i < self.chars.len() && depth > 0 {
            match self.chars[self.i] {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            self.i += 1;
        }
        let end = if depth == 0 { self.i - 1 } else { self.i };
        self.defer(start, end);
    }

    fn read_backtick(&mut self) {
        self.i += 1;
        let start = self.i;
        while self.i < self.chars.len() && self.chars[self.i] != '`' {
            self.i += 1;
        }
        let end = self.i;
        self.i += 1;
        self.defer(start, end);
    }

    fn defer(&mut self, start: usize, end: usize) {
        if end > start {
            let body: String = self.chars[start..end].iter().collect();
            self.deferred.push(body);
        }
        // A substitution stands in for a value, so the surrounding word
        // continues to exist even though its text is parsed separately.
        self.word_started = true;
    }

    fn read_input_redirect(&mut self) {
        // `<<` starts a heredoc, whose body is data and must not be parsed as
        // commands. This is what stops a file written via heredoc from being
        // classified by its own contents.
        if self.peek(1) == Some('<') {
            if self.peek(2) == Some('<') {
                // Here-string: the following word is data.
                self.i += 3;
                self.skip_redirect_target();
                return;
            }
            self.i += 2;
            if self.peek(0) == Some('-') {
                self.i += 1;
            }
            let delimiter = self.take_redirect_target();
            self.skip_heredoc_body(&delimiter);
            self.end_pipeline();
            return;
        }
        self.i += 1;
        self.skip_redirect_target();
    }

    fn read_output_redirect(&mut self) {
        // A bare fd number before `>` is part of the redirect, not a word.
        if self.word_started && self.word.chars().all(|c| c.is_ascii_digit()) {
            self.word.clear();
            self.word_started = false;
        }
        self.i += 1;
        if self.peek(0) == Some('>') {
            self.i += 1;
        }
        self.command.writes_file = true;
        self.skip_redirect_target();
    }

    fn take_redirect_target(&mut self) -> String {
        while self.i < self.chars.len() && matches!(self.chars[self.i], ' ' | '\t') {
            self.i += 1;
        }
        let mut target = String::new();
        while self.i < self.chars.len() {
            match self.chars[self.i] {
                '\'' | '"' => self.i += 1,
                c if c.is_whitespace() || matches!(c, '|' | ';' | '&' | '<' | '>') => break,
                c => {
                    target.push(c);
                    self.i += 1;
                }
            }
        }
        target
    }

    fn skip_redirect_target(&mut self) {
        let _ = self.take_redirect_target();
    }

    fn skip_heredoc_body(&mut self, delimiter: &str) {
        // Advance to the end of the current line, then drop lines until the
        // terminator. The body never becomes commands.
        while self.i < self.chars.len() && self.chars[self.i] != '\n' {
            self.i += 1;
        }
        if self.i < self.chars.len() {
            self.i += 1;
        }
        loop {
            let start = self.i;
            while self.i < self.chars.len() && self.chars[self.i] != '\n' {
                self.i += 1;
            }
            let line: String = self.chars[start..self.i].iter().collect();
            if self.i < self.chars.len() {
                self.i += 1;
            }
            if line.trim() == delimiter || start >= self.chars.len() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(raw: &str) -> Vec<String> {
        parse(raw)
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter_map(|c| c.name().map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn splits_on_operators() {
        assert_eq!(names("cd x && ls -la"), vec!["cd", "ls"]);
        assert_eq!(names("echo a; echo b"), vec!["echo", "echo"]);
        assert_eq!(names("a || b"), vec!["a", "b"]);
    }

    #[test]
    fn keeps_pipelines_together() {
        let parsed = parse("cat f | grep x | wc -l");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].commands.len(), 3);
    }

    #[test]
    fn quoted_text_is_an_argument_not_a_command() {
        // The whole point: a quoted command name must not become a command.
        let parsed = parse("seed reset \"git push --force origin main\"");
        assert_eq!(names("seed reset \"git push --force origin main\""), vec!["seed"]);
        assert_eq!(parsed[0].commands[0].argv.len(), 3);
    }

    #[test]
    fn heredoc_body_is_data() {
        let raw = "cat > out.txt <<'EOF'\nrm -rf /\nEOF\necho done";
        assert_eq!(names(raw), vec!["cat", "echo"]);
    }

    #[test]
    fn substitutions_are_parsed_and_marked() {
        let parsed = parse("echo $(whoami)");
        assert_eq!(parsed.len(), 2);
        assert!(!parsed[0].from_substitution);
        assert!(parsed[1].from_substitution);
        assert_eq!(parsed[1].commands[0].name(), Some("whoami"));
    }

    #[test]
    fn redirection_target_is_not_an_argument() {
        let parsed = parse("echo hi > out.txt");
        assert_eq!(parsed[0].commands[0].argv, vec!["echo", "hi"]);
        assert!(parsed[0].commands[0].writes_file);
    }

    #[test]
    fn detects_bundled_short_flags() {
        let parsed = parse("rm -rf build");
        assert!(parsed[0].commands[0].has_flag(&["-r"]));
        assert!(parsed[0].commands[0].has_flag(&["-f"]));
        assert!(!parsed[0].commands[0].has_flag(&["-i"]));
    }
}
