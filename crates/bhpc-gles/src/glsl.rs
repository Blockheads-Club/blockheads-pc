//! GLSL ES 1.00 (what the game's shaders are written in) for a desktop OpenGL context.
//!
//! Hosts without OpenGL ES contexts (macOS, Windows drivers without `WGL_EXT_create_context_es2_profile`) run the
//! game on a desktop compatibility context, whose GLSL 1.20 differs from GLSL ES 1.00 in little more than the
//! precision qualifiers: [`to_desktop`] drops `precision` statements and the ES-only `#version`/`#extension`
//! lines and defines `lowp`/`mediump`/`highp` away. Everything else (attribute/varying, `texture2D`,
//! `gl_FragColor`) is the same language.

/// The prologue of every translated shader.
const PROLOGUE: &str = "#version 120\n#define lowp\n#define mediump\n#define highp\n";

/// `src` (GLSL ES 1.00) as desktop GLSL 1.20.
pub fn to_desktop(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + PROLOGUE.len());
    out.push_str(PROLOGUE);
    for line in src.split_inclusive('\n') {
        let directive = line.trim_start();
        if directive.starts_with("#version") || (directive.starts_with("#extension") && directive.contains("GL_OES_")) {
            // Keep the line count, so compiler messages still point at the right line (after the prologue).
            out.push('\n');
            continue;
        }
        out.push_str(line);
    }
    strip_precision(&out)
}

/// `src` without its `precision <qualifier> <type>;` statements; comments are left alone (and never searched).
fn strip_precision(src: &str) -> String {
    const WORD: &str = "precision";
    let b = src.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(src.len());
    let (mut i, mut from) = (0, 0);
    while i < b.len() {
        if b[i..].starts_with(b"//") {
            i = src[i..].find('\n').map_or(b.len(), |n| i + n);
        } else if b[i..].starts_with(b"/*") {
            i = src[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
        } else if b[i..].starts_with(WORD.as_bytes())
            && (i == 0 || !ident(b[i - 1]))
            && b.get(i + WORD.len()).is_none_or(|c| !ident(*c))
        {
            out.push_str(&src[from..i]);
            i = src[i..].find(';').map_or(b.len(), |n| i + n + 1);
            from = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&src[from..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn es_shaders_become_desktop_glsl() {
        let fs = "#version 100\n#extension GL_OES_standard_derivatives : enable\nprecision mediump float;\n\
                  varying lowp vec2 v; uniform sampler2D t;\nvoid main() { gl_FragColor = texture2D(t, v); }\n";
        let d = to_desktop(fs);
        assert!(d.starts_with(PROLOGUE));
        let body = &d[PROLOGUE.len()..];
        assert_eq!(
            body,
            "\n\n\nvarying lowp vec2 v; uniform sampler2D t;\nvoid main() { gl_FragColor = texture2D(t, v); }\n"
        );
        // Inline statements, several per line, and identifiers that only contain the word.
        let inline = "precision mediump float; precision highp int;varying vec2 myprecision; float precision_x;";
        assert_eq!(strip_precision(inline), " varying vec2 myprecision; float precision_x;");
        // Comments are not searched; an unterminated statement or comment ends the source.
        assert_eq!(strip_precision("// precision\n/* precision; */x"), "// precision\n/* precision; */x");
        assert_eq!(strip_precision("a /* open"), "a /* open");
        assert_eq!(strip_precision("a // end"), "a // end");
        assert_eq!(strip_precision("b precision lowp float"), "b ");
        // Other extensions stay (the driver decides), and so does a source without anything to change.
        assert!(to_desktop("#extension GL_EXT_foo : enable\n").ends_with("#extension GL_EXT_foo : enable\n"));
        assert_eq!(to_desktop("void main(){}"), format!("{PROLOGUE}void main(){{}}"));
    }
}
