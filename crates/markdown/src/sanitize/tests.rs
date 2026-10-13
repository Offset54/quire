use super::*;

fn clean(html: &str) -> String {
    let mut skip = None;
    sanitize(
        html,
        &mut |u| ImageSrc::Placeholder(u.to_owned()),
        &mut skip,
    )
}

#[test]
fn scripts_handlers_and_dangerous_urls_go() {
    assert_eq!(clean("<script>alert(1)</script>ok"), "ok");
    assert_eq!(clean("<b onclick=\"x()\">b</b>"), "<b>b</b>");
    assert_eq!(
        clean("<a href=\"javascript:alert(1)\">x</a>"),
        "<a rel=\"noopener noreferrer\">x</a>"
    );
    assert_eq!(
        clean("<a href=\"&#106;avascript:x\">x</a>"),
        "<a rel=\"noopener noreferrer\">x</a>"
    );
    assert_eq!(
        clean("<a href=\" java\tscript:x\">x</a>"),
        "<a rel=\"noopener noreferrer\">x</a>"
    );
    assert_eq!(
        clean("<a href=\"https://e.com/a?b=c\">x</a>"),
        "<a href=\"https://e.com/a?b=c\" rel=\"noopener noreferrer\">x</a>"
    );
    assert_eq!(
        clean("<a href=\"docs/x.md#y\">x</a>"),
        "<a href=\"docs/x.md#y\" rel=\"noopener noreferrer\">x</a>"
    );
}

#[test]
fn frames_forms_styles_and_comments_go() {
    assert_eq!(clean("<iframe src=\"https://e.com\"></iframe>t"), "t");
    assert_eq!(
        clean("<style>body{}</style><p style=\"x\">p</p>"),
        "<p>p</p>"
    );
    assert_eq!(
        clean("<form><input type=\"text\"><button>b</button></form>"),
        "b"
    );
    assert_eq!(clean("<!-- c --><svg><script>x</script></svg>z"), "z");
    assert_eq!(clean("<object data=x>o</object>"), "");
}

#[test]
fn drop_with_content_spans_fragments() {
    let mut skip = None;
    let mut img = |u: &str| ImageSrc::Url(u.to_owned());
    assert_eq!(sanitize("<script>", &mut img, &mut skip), "");
    assert_eq!(sanitize("evil()", &mut img, &mut skip), "");
    assert_eq!(sanitize("</script>after", &mut img, &mut skip), "after");
}

#[test]
fn details_checkboxes_and_images() {
    assert_eq!(
        clean("<details open><summary>S</summary>"),
        "<details open><summary>S</summary>"
    );
    assert_eq!(
        clean("<input type=\"checkbox\" checked onclick=x>"),
        "<input type=\"checkbox\" checked disabled>"
    );
    assert_eq!(
        clean("<img src=\"https://e.com/t.png\" alt=\"a\" onerror=\"x\">"),
        "<img data-blocked-src=\"https://e.com/t.png\" alt=\"a\">"
    );
    // A broken tag is text, not markup.
    assert_eq!(
        clean("<img src=\"x onerror=y>"),
        "&lt;img src=&quot;x onerror=y&gt;"
    );
}
