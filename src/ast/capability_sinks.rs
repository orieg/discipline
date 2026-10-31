//! Calls that reach a capability sink: a process spawned, code evaluated or loaded at run
//! time, a network connection made (`Fact::CapabilitySinks`).
//!
//! An inventory for reviewers, never a finding: the review-routing section of the report
//! (`crate::routing`) lists the sites a change adds, so a reviewer knows where to look.
//! The vocabulary is closed and per language, and is matched against the callee of a call
//! node of the syntax tree, as code ([`super::calls::code_text`]): a sink named in a string
//! or a comment is not a call. Anything reached another way (an alias, a re-export, a
//! name built at run time, a method on a value) is not seen. The inventory is therefore
//! **non-exhaustive by construction**, and every surface that shows it says so.

use tree_sitter::Node;

/// What a sink gives the code that calls it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Surface {
    /// Starts another program.
    ProcessSpawn,
    /// Evaluates, compiles, imports or loads code chosen at run time.
    DynamicEval,
    /// Opens a network connection or makes a request.
    Network,
}

impl Surface {
    /// Every surface, in the order the report lists them.
    pub const ALL: [Surface; 3] = [
        Surface::ProcessSpawn,
        Surface::DynamicEval,
        Surface::Network,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Surface::ProcessSpawn => "process-spawn",
            Surface::DynamicEval => "dynamic-eval",
            Surface::Network => "network",
        }
    }
}

/// One call to a sink: its 1-based line and what it reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkSite {
    pub line: usize,
    pub surface: Surface,
}

/// One name of the vocabulary.
pub struct Entry {
    /// The callee as written, whitespace removed. A name with a `.` or `::` matches a
    /// callee that is the name or ends with it after a `.` or `:` (`std::process::Command::new`
    /// ends with `process::Command::new`); a bare name matches only the whole callee
    /// (`eval`, never `self.eval` or `my_eval`).
    pub name: &'static str,
    pub surface: Surface,
    /// The name counts only in a file one of whose import nodes holds this text: `Command::new`
    /// is a process only where `std::process` (or `tokio::process`) is imported, since
    /// `clap::Command::new` is not one.
    pub when_imported: Option<&'static str>,
}

const fn e(name: &'static str, surface: Surface) -> Entry {
    Entry {
        name,
        surface,
        when_imported: None,
    }
}

const fn imported(name: &'static str, surface: Surface, from: &'static str) -> Entry {
    Entry {
        name,
        surface,
        when_imported: Some(from),
    }
}

/// Where a language's grammar keeps its calls, imports and the sink vocabulary.
pub struct SinkSpec {
    /// Node kinds that are calls (`call_expression`, `call`, `new_expression`).
    pub call_kinds: &'static [&'static str],
    /// The field holding the callee, tried in order.
    pub callee_fields: &'static [&'static str],
    /// Node kinds that import a name (`use_declaration`, `import_from_statement`).
    pub import_kinds: &'static [&'static str],
    pub entries: &'static [Entry],
}

use Surface::{DynamicEval, Network, ProcessSpawn};

#[cfg(feature = "lang-rust")]
pub const RUST: SinkSpec = SinkSpec {
    call_kinds: &["call_expression"],
    callee_fields: &["function"],
    import_kinds: &["use_declaration"],
    entries: &[
        e("process::Command::new", ProcessSpawn),
        imported("Command::new", ProcessSpawn, "process::"),
        e("libloading::Library::new", DynamicEval),
        imported("Library::new", DynamicEval, "libloading"),
        e("TcpStream::connect", Network),
        e("TcpStream::connect_timeout", Network),
        e("TcpListener::bind", Network),
        e("UdpSocket::bind", Network),
        e("reqwest::get", Network),
        e("reqwest::Client::new", Network),
        e("reqwest::Client::builder", Network),
        e("reqwest::blocking::get", Network),
        e("reqwest::blocking::Client::new", Network),
        imported("Client::new", Network, "reqwest"),
        e("ureq::get", Network),
        e("ureq::post", Network),
        e("ureq::put", Network),
        e("ureq::delete", Network),
        e("ureq::request", Network),
        e("ureq::agent", Network),
    ],
};

#[cfg(feature = "lang-python")]
pub const PYTHON: SinkSpec = SinkSpec {
    call_kinds: &["call"],
    callee_fields: &["function"],
    import_kinds: &["import_from_statement"],
    entries: &[
        e("subprocess.run", ProcessSpawn),
        e("subprocess.Popen", ProcessSpawn),
        e("subprocess.call", ProcessSpawn),
        e("subprocess.check_call", ProcessSpawn),
        e("subprocess.check_output", ProcessSpawn),
        e("subprocess.getoutput", ProcessSpawn),
        e("subprocess.getstatusoutput", ProcessSpawn),
        imported("Popen", ProcessSpawn, "subprocess"),
        imported("check_output", ProcessSpawn, "subprocess"),
        imported("check_call", ProcessSpawn, "subprocess"),
        e("os.system", ProcessSpawn),
        e("os.popen", ProcessSpawn),
        e("os.execv", ProcessSpawn),
        e("os.execve", ProcessSpawn),
        e("os.execvp", ProcessSpawn),
        e("os.execvpe", ProcessSpawn),
        e("os.execl", ProcessSpawn),
        e("os.execlp", ProcessSpawn),
        e("os.spawnv", ProcessSpawn),
        e("os.spawnl", ProcessSpawn),
        e("os.posix_spawn", ProcessSpawn),
        e("os.posix_spawnp", ProcessSpawn),
        e("asyncio.create_subprocess_exec", ProcessSpawn),
        e("asyncio.create_subprocess_shell", ProcessSpawn),
        e("pty.spawn", ProcessSpawn),
        e("eval", DynamicEval),
        e("exec", DynamicEval),
        e("compile", DynamicEval),
        e("__import__", DynamicEval),
        e("importlib.import_module", DynamicEval),
        imported("import_module", DynamicEval, "importlib"),
        e("runpy.run_path", DynamicEval),
        e("runpy.run_module", DynamicEval),
        e("pickle.loads", DynamicEval),
        e("pickle.load", DynamicEval),
        e("marshal.loads", DynamicEval),
        e("ctypes.CDLL", DynamicEval),
        e("cdll.LoadLibrary", DynamicEval),
        e("urllib.request.urlopen", Network),
        e("request.urlopen", Network),
        imported("urlopen", Network, "urllib"),
        e("requests.get", Network),
        e("requests.post", Network),
        e("requests.put", Network),
        e("requests.patch", Network),
        e("requests.delete", Network),
        e("requests.head", Network),
        e("requests.request", Network),
        e("requests.Session", Network),
        e("httpx.get", Network),
        e("httpx.post", Network),
        e("httpx.put", Network),
        e("httpx.patch", Network),
        e("httpx.delete", Network),
        e("httpx.request", Network),
        e("httpx.Client", Network),
        e("httpx.AsyncClient", Network),
        e("aiohttp.ClientSession", Network),
        e("socket.socket", Network),
        e("socket.create_connection", Network),
        e("http.client.HTTPConnection", Network),
        e("http.client.HTTPSConnection", Network),
        e("ftplib.FTP", Network),
        e("smtplib.SMTP", Network),
    ],
};

#[cfg(feature = "lang-javascript")]
pub const JAVASCRIPT: SinkSpec = SinkSpec {
    call_kinds: &["call_expression", "new_expression"],
    callee_fields: &["function", "constructor"],
    import_kinds: &["import_statement"],
    entries: &[
        e("child_process.exec", ProcessSpawn),
        e("child_process.execSync", ProcessSpawn),
        e("child_process.execFile", ProcessSpawn),
        e("child_process.execFileSync", ProcessSpawn),
        e("child_process.spawn", ProcessSpawn),
        e("child_process.spawnSync", ProcessSpawn),
        e("child_process.fork", ProcessSpawn),
        e("execSync", ProcessSpawn),
        e("execFileSync", ProcessSpawn),
        e("spawnSync", ProcessSpawn),
        imported("exec", ProcessSpawn, "child_process"),
        imported("execFile", ProcessSpawn, "child_process"),
        imported("spawn", ProcessSpawn, "child_process"),
        imported("fork", ProcessSpawn, "child_process"),
        e("Deno.run", ProcessSpawn),
        e("Deno.Command", ProcessSpawn),
        e("Bun.spawn", ProcessSpawn),
        e("Bun.spawnSync", ProcessSpawn),
        e("execa", ProcessSpawn),
        e("eval", DynamicEval),
        e("Function", DynamicEval),
        e("import", DynamicEval),
        e("vm.runInNewContext", DynamicEval),
        e("vm.runInThisContext", DynamicEval),
        e("vm.runInContext", DynamicEval),
        e("vm.compileFunction", DynamicEval),
        e("vm.Script", DynamicEval),
        e("fetch", Network),
        e("http.request", Network),
        e("http.get", Network),
        e("https.request", Network),
        e("https.get", Network),
        e("net.connect", Network),
        e("net.createConnection", Network),
        e("tls.connect", Network),
        e("dgram.createSocket", Network),
        e("axios", Network),
        e("axios.get", Network),
        e("axios.post", Network),
        e("axios.put", Network),
        e("axios.patch", Network),
        e("axios.delete", Network),
        e("axios.request", Network),
        e("WebSocket", Network),
        e("XMLHttpRequest", Network),
        e("EventSource", Network),
    ],
};

#[cfg(feature = "lang-go")]
pub const GO: SinkSpec = SinkSpec {
    call_kinds: &["call_expression"],
    callee_fields: &["function"],
    import_kinds: &[],
    entries: &[
        e("exec.Command", ProcessSpawn),
        e("exec.CommandContext", ProcessSpawn),
        e("os.StartProcess", ProcessSpawn),
        e("syscall.Exec", ProcessSpawn),
        e("syscall.ForkExec", ProcessSpawn),
        e("plugin.Open", DynamicEval),
        e("http.Get", Network),
        e("http.Post", Network),
        e("http.PostForm", Network),
        e("http.Head", Network),
        e("http.NewRequest", Network),
        e("http.NewRequestWithContext", Network),
        e("net.Dial", Network),
        e("net.DialTimeout", Network),
        e("net.DialTCP", Network),
        e("net.DialUDP", Network),
        e("net.Listen", Network),
        e("tls.Dial", Network),
        e("grpc.Dial", Network),
        e("grpc.NewClient", Network),
    ],
};

/// Whether `callee` (whitespace removed) is `name` as [`Entry::name`] describes.
fn names(callee: &str, name: &str) -> bool {
    if callee == name {
        return true;
    }
    if !name.contains(['.', ':']) {
        return false;
    }
    callee
        .strip_suffix(name)
        .is_some_and(|before| before.ends_with(['.', ':']))
}

/// The sink calls of a parsed file, in source order. `spec` is the file's language.
pub fn sites(root: Node, src: &str, spec: &SinkSpec) -> Vec<SinkSite> {
    let mut calls: Vec<(usize, String)> = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        super::ancestry::count(1);
        if spec.import_kinds.contains(&node.kind()) {
            imports.push(super::calls::code_text(node, src));
        }
        if spec.call_kinds.contains(&node.kind()) {
            if let Some(callee) = spec
                .callee_fields
                .iter()
                .find_map(|f| node.child_by_field_name(f))
            {
                let text: String = super::calls::code_text(callee, src)
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                calls.push((node.start_position().row + 1, text));
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    let imported = |from: &str| imports.iter().any(|i| i.contains(from));
    let mut out = Vec::new();
    for (line, callee) in calls {
        let hit = spec
            .entries
            .iter()
            .find(|entry| names(&callee, entry.name) && entry.when_imported.is_none_or(&imported));
        if let Some(entry) = hit {
            out.push(SinkSite {
                line,
                surface: entry.surface,
            });
        }
    }
    out.sort_by_key(|s| s.line);
    out
}

/// [`sites`] of `src`, parsed with `language`. A file that does not parse is an error the
/// caller names as not inventoried.
pub fn sites_of(
    language: &tree_sitter::Language,
    grammar: &str,
    path: &str,
    src: &str,
    spec: &SinkSpec,
) -> anyhow::Result<Vec<SinkSite>> {
    let tree = super::source_text::parse_file_as(language, grammar, path, src)?;
    Ok(sites(tree.root_node(), src, spec))
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-go"
))]
mod tests {
    use super::*;
    use crate::ast::default_registry;

    fn found(path: &str, src: &str) -> Vec<(usize, &'static str)> {
        let reg = default_registry();
        let pack = reg.find_pack(path).expect("a pack");
        assert!(pack.supplies(crate::ast::Fact::CapabilitySinks));
        pack.capability_sinks(path, src)
            .expect("parses")
            .into_iter()
            .map(|s| (s.line, s.surface.as_str()))
            .collect()
    }

    #[test]
    fn rust_sinks_are_read_from_calls_and_need_the_import() {
        let src = "use std::process::Command;\nfn a() {\n    let _ = Command::new(\"git\");\n    let _ = std::net::TcpStream::connect(\"h:1\");\n    // Command::new(\"x\") in a comment\n    let _ = \"reqwest::get(\";\n}\n";
        assert_eq!(
            found("src/a.rs", src),
            vec![(3, "process-spawn"), (4, "network")]
        );
        // `clap::Command::new` is not a process: no `process::` import.
        let clap = "use clap::Command;\nfn a() { let _ = Command::new(\"cli\"); }\n";
        assert!(found("src/b.rs", clap).is_empty());
        // A name that ends like an entry is not one.
        let other = "fn a() { let _ = MyCommand::new(); let _ = my_reqwest::get(); }\n";
        assert!(found("src/c.rs", other).is_empty());
    }

    #[test]
    fn python_sinks_cover_the_three_surfaces() {
        let src = "import subprocess\nfrom importlib import import_module\nsubprocess.run(['ls'])\neval(x)\nself.eval(x)\nimport_module(n)\nrequests.get(u)\nprint('os.system(x)')\n";
        assert_eq!(
            found("tools/a.py", src),
            vec![
                (3, "process-spawn"),
                (4, "dynamic-eval"),
                (6, "dynamic-eval"),
                (7, "network")
            ]
        );
    }

    #[test]
    fn javascript_sinks_read_new_expressions_and_dynamic_import() {
        let src = "const cp = require('child_process');\nchild_process.execSync('ls');\nnew Function('return 1');\nawait fetch(u);\nconst m = await import(p);\nregex.exec(s);\n";
        assert_eq!(
            found("src/a.ts", src),
            vec![
                (2, "process-spawn"),
                (3, "dynamic-eval"),
                (4, "network"),
                (5, "dynamic-eval")
            ]
        );
    }

    #[test]
    fn go_sinks_are_package_qualified_calls() {
        let src = "package a\nimport \"os/exec\"\nfunc f() {\n\texec.Command(\"ls\")\n\thttp.Get(u)\n\tother.Command(\"x\")\n}\n";
        assert_eq!(
            found("a.go", src),
            vec![(4, "process-spawn"), (5, "network")]
        );
    }

    #[test]
    fn packs_without_the_fact_report_none() {
        let reg = default_registry();
        let java = reg.find_pack("A.java").unwrap();
        assert!(!java.supplies(crate::ast::Fact::CapabilitySinks));
        assert!(java
            .capability_sinks(
                "A.java",
                "class A { void f() { Runtime.getRuntime().exec(\"x\"); } }"
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_qualified_name_matches_after_a_separator_only() {
        assert!(names("std::process::Command::new", "process::Command::new"));
        assert!(names("subprocess.run", "subprocess.run"));
        assert!(!names("mysubprocess.run", "subprocess.run"));
        assert!(!names("self.eval", "eval"));
        assert!(names("eval", "eval"));
    }
}
