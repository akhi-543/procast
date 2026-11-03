//! Raycast-format script commands: any script whose comment header has `@raycast.*` keys.
//! https://github.com/raycast/script-commands documents the format.

use std::{fs, path::Path, process::Command};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    /// Run, close the launcher, report only failures.
    Silent,
    /// Keep the launcher open and show the last output line.
    Compact,
    /// Show all output as a list.
    Full,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub placeholder: String,
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub title: String,
    pub path: String,
    pub output: Output,
    pub package: String,
    pub icon: Option<String>,
    pub args: Vec<Arg>,
    pub confirm: bool,
}

/// Reads every script header in `dirs` (top level only, like Raycast).
pub fn scan(dirs: &[String]) -> Vec<Script> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(read) = fs::read_dir(dir) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_file()
                && let Ok(text) = fs::read_to_string(&path)
                && let Some(script) = parse(&path.to_string_lossy(), &text)
            {
                out.push(script);
            }
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

pub fn parse(path: &str, text: &str) -> Option<Script> {
    let mut s = Script { title: String::new(), path: path.into(), output: Output::Full, package: String::new(), icon: None, args: vec![], confirm: false };
    for line in text.lines().take(40) {
        let Some((_, rest)) = line.split_once("@raycast.") else { continue };
        let (key, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let value = value.trim();
        match key {
            "title" => s.title = value.into(),
            "packageName" => s.package = value.into(),
            "icon" if !value.contains(['/', '\\', '.']) => s.icon = Some(value.into()),
            "needsConfirmation" => s.confirm = value == "true",
            "mode" => {
                s.output = match value {
                    "silent" => Output::Silent,
                    "compact" | "inline" => Output::Compact,
                    _ => Output::Full,
                }
            }
            k if k.starts_with("argument") => {
                let v: serde_json::Value = serde_json::from_str(value).ok()?;
                s.args.push(Arg {
                    placeholder: v["placeholder"].as_str().unwrap_or("Argument").into(),
                    optional: v["optional"].as_bool().unwrap_or(false),
                });
            }
            _ => {}
        }
    }
    (!s.title.is_empty()).then_some(s)
}

/// The interpreter for a script, from its extension or `#!` line.
pub fn command(script: &Script, args: &[String]) -> Command {
    let path = Path::new(&script.path);
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let with = |prog: &str, pre: &[&str]| {
        let mut c = Command::new(prog);
        c.args(pre).arg(&script.path);
        c
    };
    let mut cmd = match ext.as_str() {
        "ps1" => with("powershell", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]),
        "py" => with("python", &[]),
        "js" | "mjs" => with("node", &[]),
        "bat" | "cmd" => with("cmd", &["/c"]),
        _ => {
            let shebang = fs::read_to_string(path).ok().and_then(|t| t.lines().next().map(str::to_string)).unwrap_or_default();
            match shebang.strip_prefix("#!").and_then(|s| s.split_whitespace().last()) {
                Some(prog) => with(prog.rsplit('/').next().unwrap_or(prog), &[]),
                None => Command::new(&script.path),
            }
        }
    };
    cmd.args(args);
    if let Some(dir) = path.parent() {
        cmd.current_dir(dir);
    }
    cmd
}

/// Runs to completion, returning (succeeded, combined output).
pub fn run(script: &Script, args: &[String]) -> (bool, String) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match command(script, args).creation_flags(CREATE_NO_WINDOW).output() {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).trim_end().to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                text = format!("{text}\n{}", err.trim_end()).trim_start().to_string();
            }
            (o.status.success(), text)
        }
        Err(e) => (false, format!("could not start {}: {e}", script.path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_raycast_headers() {
        let text = "#!/usr/bin/env python3\n\
            # Required parameters:\n\
            # @raycast.schemaVersion 1\n\
            # @raycast.title Convert Case\n\
            # @raycast.mode compact\n\
            # @raycast.packageName Text\n\
            # @raycast.icon 🔤\n\
            # @raycast.argument1 { \"type\": \"text\", \"placeholder\": \"Text\" }\n\
            # @raycast.argument2 { \"type\": \"text\", \"placeholder\": \"Style\", \"optional\": true }\n\
            # @raycast.needsConfirmation true\n";
        let s = parse("C:\\s\\case.py", text).unwrap();
        assert_eq!((s.title.as_str(), s.output, s.package.as_str()), ("Convert Case", Output::Compact, "Text"));
        assert_eq!(s.icon.as_deref(), Some("🔤"));
        assert_eq!(s.args, vec![Arg { placeholder: "Text".into(), optional: false }, Arg { placeholder: "Style".into(), optional: true }]);
        assert!(s.confirm);
        // PowerShell-style comments work too; no title means not a script command.
        assert!(parse("x.ps1", "# @raycast.title Hi\n# @raycast.mode silent").is_some_and(|s| s.output == Output::Silent));
        assert!(parse("x.ps1", "Write-Host hi").is_none());
        let cmd = command(&s, &["a".into()]);
        assert_eq!(cmd.get_program(), "python");
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["C:\\s\\case.py", "a"]);
        let ps = command(&parse("C:\\s\\x.ps1", "# @raycast.title X").unwrap(), &[]);
        assert_eq!(ps.get_program(), "powershell");
        assert_eq!(ps.get_args().last().unwrap(), "C:\\s\\x.ps1");
    }
}
