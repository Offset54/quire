//! The grammars (M2 decision D) and their queries, as each crate ships
//! them. C++ and TypeScript extend C's and JavaScript's queries, as their
//! upstream `tree-sitter.json` files do.

use std::sync::OnceLock;

use tree_sitter_highlight::HighlightConfiguration;

use crate::NAMES;

/// A grammar and how to highlight with it.
pub struct Language {
    /// Names and aliases a fence's info string may use (lowercase); the
    /// first is the canonical one.
    pub names: &'static [&'static str],
    /// Only reached through another grammar's injections (Markdown's
    /// inline grammar).
    pub hidden: bool,
    /// A short sample (tests).
    pub sample: &'static str,
    build: fn() -> HighlightConfiguration,
    config: OnceLock<HighlightConfiguration>,
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Language")
            .field("name", &self.names[0])
            .finish()
    }
}

impl Language {
    /// Whether the configuration is built (building it can take a few
    /// hundred milliseconds).
    pub fn is_ready(&self) -> bool {
        self.config.get().is_some()
    }

    /// The highlight configuration, built on first use.
    pub fn config(&'static self) -> &'static HighlightConfiguration {
        self.config.get_or_init(|| {
            let mut c = (self.build)();
            c.configure(NAMES);
            c
        })
    }
}

fn config(
    lang: tree_sitter_language::LanguageFn,
    name: &str,
    highlights: &str,
    injections: &str,
    locals: &str,
) -> HighlightConfiguration {
    // The queries ship with the grammars and are checked by the tests: a
    // failure here is a build defect.
    HighlightConfiguration::new(lang.into(), name, highlights, injections, locals)
        .unwrap_or_else(|e| panic!("highlight query for {name}: {e}"))
}

macro_rules! lang {
    ($names:expr, $sample:expr, $build:expr) => {
        Language {
            names: $names,
            hidden: false,
            sample: $sample,
            build: $build,
            config: OnceLock::new(),
        }
    };
}

fn concat(parts: &[&str]) -> String {
    parts.join("\n")
}

/// Every grammar, in decision D's order.
pub static LANGUAGES: [Language; 23] = [
    lang!(
        &["bash", "sh", "shell", "zsh", "shellscript", "console"],
        "for f in *.md; do echo \"$f\" # list\ndone\n",
        || config(
            tree_sitter_bash::LANGUAGE,
            "bash",
            tree_sitter_bash::HIGHLIGHT_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["c", "h"],
        "#include <stdio.h>\nint main(void) { return 0; } // done\n",
        || config(
            tree_sitter_c::LANGUAGE,
            "c",
            tree_sitter_c::HIGHLIGHT_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["cpp", "c++", "cc", "cxx", "hpp", "hh"],
        "template <typename T> class Box { T v; };\nauto x = std::make_unique<int>(1);\n",
        || config(
            tree_sitter_cpp::LANGUAGE,
            "cpp",
            &concat(&[
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            ]),
            "",
            ""
        )
    ),
    lang!(
        &["csharp", "c#", "cs"],
        "public class A { public string Name { get; set; } = \"x\"; }\n",
        || config(
            tree_sitter_c_sharp::LANGUAGE,
            "csharp",
            tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["css"],
        "a:hover { color: #fff; margin: 0 4px; } /* note */\n",
        || config(
            tree_sitter_css::LANGUAGE,
            "css",
            tree_sitter_css::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["go", "golang"],
        "package main\n\nfunc main() { fmt.Println(\"hi\") }\n",
        || config(
            tree_sitter_go::LANGUAGE,
            "go",
            tree_sitter_go::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["html", "htm", "xhtml"],
        "<p class=\"x\">Hi &amp; bye</p>\n<script>let a = 1;</script>\n",
        || config(
            tree_sitter_html::LANGUAGE,
            "html",
            tree_sitter_html::HIGHLIGHTS_QUERY,
            tree_sitter_html::INJECTIONS_QUERY,
            ""
        )
    ),
    lang!(
        &["java"],
        "public class A { public static void main(String[] a) { int x = 1; } }\n",
        || config(
            tree_sitter_java::LANGUAGE,
            "java",
            tree_sitter_java::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["javascript", "js", "jsx", "mjs", "cjs", "node"],
        "const f = async (x) => { return `v ${x}`; }; // c\n",
        || config(
            tree_sitter_javascript::LANGUAGE,
            "javascript",
            &concat(&[
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_javascript::HIGHLIGHT_QUERY,
            ]),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY
        )
    ),
    lang!(
        &["json", "jsonc", "json5"],
        "{\"a\": [1, 2.5, true, null], \"b\": \"x\"}\n",
        || config(
            tree_sitter_json::LANGUAGE,
            "json",
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["kotlin", "kt", "kts"],
        "fun main() { val x: Int = 1; println(\"$x\") }\n",
        || config(
            tree_sitter_kotlin_sg::LANGUAGE,
            "kotlin",
            tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["markdown", "md"],
        "# Title\n\nSome *text* and `code`.\n\n```rust\nfn x() {}\n```\n",
        || config(
            tree_sitter_md::LANGUAGE,
            "markdown",
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            tree_sitter_md::INJECTION_QUERY_BLOCK,
            ""
        )
    ),
    lang!(
        &["powershell", "ps1", "ps", "pwsh", "psm1"],
        "$x = Get-ChildItem -Path . | Where-Object { $_.Length -gt 1 } # c\n",
        || config(
            tree_sitter_powershell::LANGUAGE,
            "powershell",
            tree_sitter_powershell::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["python", "py", "py3", "python3"],
        "def f(x: int) -> str:\n    return f\"{x}\"  # c\n",
        || config(
            tree_sitter_python::LANGUAGE,
            "python",
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["ruby", "rb"],
        "class A\n  def x(y) = \"#{y}\" # c\nend\n",
        || config(
            tree_sitter_ruby::LANGUAGE,
            "ruby",
            tree_sitter_ruby::HIGHLIGHTS_QUERY,
            "",
            tree_sitter_ruby::LOCALS_QUERY
        )
    ),
    lang!(
        &["rust", "rs"],
        "fn main() { let x: u32 = 1; println!(\"{x}\"); } // c\n",
        || config(
            tree_sitter_rust::LANGUAGE,
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            ""
        )
    ),
    lang!(
        &["sql", "mysql", "postgres", "postgresql", "sqlite", "pgsql"],
        "SELECT a, COUNT(*) FROM t WHERE b = 'x' GROUP BY a; -- c\n",
        || config(
            tree_sitter_sequel::LANGUAGE,
            "sql",
            tree_sitter_sequel::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["swift"],
        "struct A { let x: Int = 1 }\nfunc f() -> String { \"\\(1)\" }\n",
        || config(
            tree_sitter_swift::LANGUAGE,
            "swift",
            tree_sitter_swift::HIGHLIGHTS_QUERY,
            "",
            tree_sitter_swift::LOCALS_QUERY
        )
    ),
    lang!(
        &["toml"],
        "[package]\nname = \"quire\" # c\nversion = 1\n",
        || config(
            tree_sitter_toml_ng::LANGUAGE,
            "toml",
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            "",
            ""
        )
    ),
    lang!(
        &["typescript", "ts", "mts", "cts"],
        "interface A { x: number }\nconst a: A = { x: 1 } as const;\n",
        || config(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            "typescript",
            &concat(&[
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                tree_sitter_javascript::HIGHLIGHT_QUERY,
            ]),
            tree_sitter_javascript::INJECTIONS_QUERY,
            &concat(&[
                tree_sitter_typescript::LOCALS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            ])
        )
    ),
    lang!(
        &["tsx"],
        "const A = (p: { x: number }) => <div className=\"a\">{p.x}</div>;\n",
        || config(
            tree_sitter_typescript::LANGUAGE_TSX,
            "tsx",
            &concat(&[
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_javascript::HIGHLIGHT_QUERY,
            ]),
            tree_sitter_javascript::INJECTIONS_QUERY,
            &concat(&[
                tree_sitter_typescript::LOCALS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            ])
        )
    ),
    lang!(&["yaml", "yml"], "a: 1\nb: [x, \"y\"] # c\n", || config(
        tree_sitter_yaml::LANGUAGE,
        "yaml",
        tree_sitter_yaml::HIGHLIGHTS_QUERY,
        "",
        ""
    )),
    Language {
        names: &["markdown_inline"],
        hidden: true,
        sample: "Some *text* and `code` and [a link](x).\n",
        build: || {
            config(
                tree_sitter_md::INLINE_LANGUAGE,
                "markdown_inline",
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                tree_sitter_md::INJECTION_QUERY_INLINE,
                "",
            )
        },
        config: OnceLock::new(),
    },
];

/// The grammar for a fence's language name or alias (any case).
pub fn language(name: &str) -> Option<&'static Language> {
    let name = name.trim();
    if name.is_empty() || name.len() > 32 {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    LANGUAGES.iter().find(|l| l.names.contains(&lower.as_str()))
}
