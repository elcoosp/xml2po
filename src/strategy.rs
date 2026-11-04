//! Format strategies: per-format rules deciding what is translatable.

use crate::error::{Error, Result};
use crate::filter;
use crate::options::ExtractionOptions;

/// Per-format extraction rules.
///
/// A strategy tells the extractor which elements carry translatable text,
/// which elements must be skipped entirely, and which *attribute* values
/// are translatable. Implement this trait to support a custom format;
/// the built-ins cover MJML, HTML and generic XML.
pub trait ExtractionStrategy: Send + Sync {
    /// Lowercase canonical name of the format (used by `--format`).
    fn format_name(&self) -> &'static str;

    /// Canonical file extension including the leading dot.
    fn file_extension(&self) -> &'static str;

    /// Whether the *text content* of this element is translatable.
    fn is_translatable_element(&self, element: &str) -> bool;

    /// Whether this element must be ignored altogether (its text and
    /// attributes are never extracted). Defaults to `false`.
    fn is_ignored_element(&self, _element: &str) -> bool {
        false
    }

    /// Names of attributes whose values are translatable for this format.
    /// Defaults to no attributes.
    fn translatable_attributes(&self) -> &[&str] {
        &[]
    }

    /// Whether this element is *inline*: when inline placeholders are
    /// enabled it folds into the surrounding message as a `<tag>`
    /// placeholder instead of splitting the segment. Block-level and
    /// unknown elements split the segment like before. Defaults to
    /// `false`, so custom strategies keep the classic behavior until
    /// they opt in.
    fn is_inline_element(&self, _element: &str) -> bool {
        false
    }

    /// Whether a text node is translatable. Defaults to the shared
    /// heuristic in [`crate::filter`].
    fn is_translatable_text(&self, text: &str) -> bool {
        filter::is_translatable_text(text)
    }

    /// Resolves the effective element set by merging the strategy rules
    /// with user-supplied [`ExtractionOptions`].
    fn resolve(
        &self,
        element: &str,
        attribute: Option<&str>,
        options: &ExtractionOptions,
    ) -> ResolvedRule {
        let extra = options.extra_translatable_elements.as_slice();
        let ignored = self.is_ignored_element(element)
            || options.ignored_elements.iter().any(|e| e == element);
        let attr_translatable = attribute.is_some_and(|a| {
            self.translatable_attributes().contains(&a)
                || options.translatable_attributes.iter().any(|x| x == a)
        });
        let text_translatable = !ignored
            && (self.is_translatable_element(element)
                || extra.iter().any(|e| e == element)
                || attr_translatable);
        ResolvedRule {
            text_translatable,
            ignored,
            attribute_translatable: attr_translatable,
            inline: self.is_inline_element(element),
        }
    }
}

/// Outcome of merging a strategy with user options for one element.
///
/// The four flags are intentionally independent: every combination is
/// meaningful, so grouping them would obscure the trait contract.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRule {
    /// Text content of the element (or the attribute value) is translatable.
    pub text_translatable: bool,
    /// Element is skipped entirely.
    pub ignored: bool,
    /// The attribute (if any) is translatable.
    pub attribute_translatable: bool,
    /// Element folds into the surrounding message as an inline
    /// placeholder (see [`ExtractionStrategy::is_inline_element`]).
    pub inline: bool,
}

/// Value of the HTML5/ITS `translate` attribute: `Some(true)` for
/// `translate="yes"`, `Some(false)` for `translate="no"`, `None` when
/// absent. Both the plain `translate` attribute and the namespaced
/// `its:translate` form are recognized.
pub(crate) fn translate_directive(attributes: &[(String, String)]) -> Option<bool> {
    attributes.iter().find_map(|(name, value)| {
        if name.eq_ignore_ascii_case("translate") || name.eq_ignore_ascii_case("its:translate") {
            match value.trim().to_ascii_lowercase().as_str() {
                "no" => Some(false),
                "yes" => Some(true),
                _ => None,
            }
        } else {
            None
        }
    })
}

/// Merges a strategy's rule for one element with the element-level
/// `translate="yes|no"` directive.
///
/// `translate="no"` behaves like an ignored element (nothing inside is
/// extracted or translated); `translate="yes"` forces the text
/// translatable unless the element is ignored altogether.
pub(crate) fn effective_rule(
    strategy: &dyn ExtractionStrategy,
    element: &str,
    translate: Option<bool>,
    options: &ExtractionOptions,
) -> ResolvedRule {
    let mut rule = strategy.resolve(element, None, options);
    if options.respect_translate_attribute {
        match translate {
            Some(false) => {
                rule.ignored = true;
                rule.text_translatable = false;
                rule.attribute_translatable = false;
            }
            Some(true) if !rule.ignored => {
                rule.text_translatable = true;
            }
            _ => {}
        }
    }
    rule
}

/// The MJML e-mail framework: `mj-title`, `mj-text`, `mj-button` plus
/// inline `strong`/`em` emphasis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MjmlStrategy;

impl ExtractionStrategy for MjmlStrategy {
    fn format_name(&self) -> &'static str {
        "mjml"
    }

    fn file_extension(&self) -> &'static str {
        ".mjml"
    }

    fn is_translatable_element(&self, element: &str) -> bool {
        matches!(
            element,
            "mj-title" | "mj-text" | "mj-button" | "strong" | "em"
        )
    }

    fn is_ignored_element(&self, element: &str) -> bool {
        matches!(element, "mj-style" | "style" | "script")
    }

    fn is_inline_element(&self, element: &str) -> bool {
        matches!(element, "strong" | "em")
    }
}

/// HTML documents: classic text flow elements plus the `alt`, `title`,
/// `placeholder` and `aria-label` attributes. `script` and `style` are
/// always ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HtmlStrategy;

impl ExtractionStrategy for HtmlStrategy {
    fn format_name(&self) -> &'static str {
        "html"
    }

    fn file_extension(&self) -> &'static str {
        ".html"
    }

    fn is_translatable_element(&self, element: &str) -> bool {
        matches!(
            element,
            "p" | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "span"
                | "div"
                | "a"
                | "button"
                | "label"
                | "li"
                | "td"
                | "th"
                | "title"
                | "option"
                | "strong"
                | "em"
                | "b"
                | "i"
        )
    }

    fn is_ignored_element(&self, element: &str) -> bool {
        matches!(element, "script" | "style" | "code" | "pre")
    }

    fn translatable_attributes(&self) -> &[&str] {
        &["alt", "title", "placeholder", "aria-label"]
    }

    /// HTML phrasing content folds into the surrounding message.
    /// `code`/`pre`/`script`/`style` stay ignored, so they never fold.
    fn is_inline_element(&self, element: &str) -> bool {
        matches!(
            element,
            "a" | "abbr"
                | "b"
                | "bdi"
                | "bdo"
                | "br"
                | "cite"
                | "data"
                | "dfn"
                | "em"
                | "i"
                | "kbd"
                | "mark"
                | "q"
                | "rp"
                | "rt"
                | "ruby"
                | "s"
                | "samp"
                | "small"
                | "span"
                | "strong"
                | "sub"
                | "sup"
                | "time"
                | "u"
                | "var"
                | "wbr"
        )
    }
}

/// Generic XML: every element's text content is translatable unless it is
/// ignored, which makes the strategy useful for configuration files,
/// SVG, Android resources, XLIFF-like documents, …
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct XmlStrategy;

impl ExtractionStrategy for XmlStrategy {
    fn format_name(&self) -> &'static str {
        "xml"
    }

    fn file_extension(&self) -> &'static str {
        ".xml"
    }

    fn is_translatable_element(&self, _element: &str) -> bool {
        true
    }

    fn is_ignored_element(&self, element: &str) -> bool {
        matches!(element, "script" | "style")
    }
}

/// Built-in format identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// MJML e-mail templates.
    Mjml,
    /// HTML documents.
    Html,
    /// Generic XML documents.
    Xml,
}

impl Format {
    /// The strategy instance for this format.
    #[must_use]
    pub fn strategy(self) -> &'static dyn ExtractionStrategy {
        match self {
            Self::Mjml => &MjmlStrategy,
            Self::Html => &HtmlStrategy,
            Self::Xml => &XmlStrategy,
        }
    }

    /// Canonical lowercase name.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.strategy().format_name()
    }
}

/// All format names compiled into this build.
#[must_use]
pub fn available_formats() -> Vec<&'static str> {
    [
        ("mjml", cfg!(feature = "mjml")),
        ("html", cfg!(feature = "html")),
        ("xml", cfg!(feature = "xml")),
    ]
    .into_iter()
    .filter_map(|(name, enabled)| enabled.then_some(name))
    .collect()
}

/// Resolves a format *name* (case-insensitive) to the built-in format.
///
/// # Errors
///
/// Returns [`Error::UnsupportedFormat`] when the name matches no
/// compiled-in format.
pub fn get_format(name: &str) -> Result<Format> {
    match name.to_ascii_lowercase().as_str() {
        #[cfg(feature = "mjml")]
        "mjml" => Ok(Format::Mjml),
        #[cfg(feature = "html")]
        "html" => Ok(Format::Html),
        #[cfg(feature = "xml")]
        "xml" => Ok(Format::Xml),
        _ => Err(Error::unsupported_format(name, &available_formats())),
    }
}

/// Resolves a format name to a static strategy reference.
///
/// Static promotion avoids a per-call allocation and lets callers pass
/// `&dyn ExtractionStrategy` around cheaply.
///
/// # Errors
///
/// Returns [`Error::UnsupportedFormat`] when the name matches no
/// compiled-in format.
pub fn get_strategy(format: &str) -> Result<&'static dyn ExtractionStrategy> {
    get_format(format).map(Format::strategy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_metadata() {
        assert_eq!(MjmlStrategy.format_name(), "mjml");
        assert_eq!(MjmlStrategy.file_extension(), ".mjml");
        assert_eq!(HtmlStrategy.file_extension(), ".html");
        assert_eq!(XmlStrategy.file_extension(), ".xml");
    }

    #[test]
    fn mjml_rules() {
        let s = MjmlStrategy;
        for el in ["mj-title", "mj-text", "mj-button", "strong", "em"] {
            assert!(s.is_translatable_element(el), "{el}");
        }
        for el in ["mj-section", "mj-column", "mj-image", "div"] {
            assert!(!s.is_translatable_element(el), "{el}");
        }
        for el in ["mj-style", "style", "script"] {
            assert!(s.is_ignored_element(el), "{el}");
        }
        assert_eq!(s.translatable_attributes(), &[] as &[&str]);
    }

    #[test]
    fn html_rules() {
        let s = HtmlStrategy;
        for el in ["p", "h1", "h6", "a", "button", "title", "i"] {
            assert!(s.is_translatable_element(el), "{el}");
        }
        for el in ["html", "body", "head", "meta", "img"] {
            assert!(!s.is_translatable_element(el), "{el}");
        }
        for el in ["script", "style", "code", "pre"] {
            assert!(s.is_ignored_element(el), "{el}");
        }
        for attr in ["alt", "title", "placeholder", "aria-label"] {
            assert!(s.translatable_attributes().contains(&attr), "{attr}");
        }
    }

    #[test]
    fn xml_rules() {
        let s = XmlStrategy;
        assert!(s.is_translatable_element("anything"));
        assert!(s.is_ignored_element("script"));
        assert!(!s.is_ignored_element("label"));
        assert_eq!(s.translatable_attributes(), &[] as &[&str]);
    }

    #[test]
    fn resolve_merges_options() {
        let options = ExtractionOptions::builder()
            .extra_translatable_element("custom-widget")
            .ignored_element("div")
            .translatable_attribute("data-label")
            .build();
        let s = HtmlStrategy;

        // Extra element becomes translatable.
        assert!(s.resolve("custom-widget", None, &options).text_translatable);
        // Strategy default still applies.
        assert!(s.resolve("p", None, &options).text_translatable);
        // Ignored via options wins over strategy default.
        let div = s.resolve("div", None, &options);
        assert!(div.ignored);
        assert!(!div.text_translatable);
        // Option-provided attribute is translatable.
        let attr = s.resolve("img", Some("data-label"), &options);
        assert!(attr.attribute_translatable);
        assert!(attr.text_translatable);
        // Unknown attribute is not.
        assert!(
            !s.resolve("img", Some("src"), &options)
                .attribute_translatable
        );
    }

    #[test]
    fn resolve_default_options_keeps_defaults() {
        let options = ExtractionOptions::default();
        assert!(HtmlStrategy.resolve("p", None, &options).text_translatable);
        assert!(
            HtmlStrategy
                .resolve("img", Some("alt"), &options)
                .attribute_translatable
        );
        assert!(
            !HtmlStrategy
                .resolve("img", Some("src"), &options)
                .attribute_translatable
        );
    }

    /// A custom strategy demonstrating the trait defaults.
    struct Minimal;

    impl ExtractionStrategy for Minimal {
        fn format_name(&self) -> &'static str {
            "minimal"
        }

        fn file_extension(&self) -> &'static str {
            ".min"
        }

        fn is_translatable_element(&self, element: &str) -> bool {
            element == "msg"
        }
    }

    #[test]
    fn custom_strategies_use_trait_defaults() {
        let s = Minimal;
        assert_eq!(s.format_name(), "minimal");
        assert_eq!(s.file_extension(), ".min");
        assert_eq!(s.translatable_attributes(), &[] as &[&str]);
        assert!(!s.is_ignored_element("anything"));
        assert!(s.is_translatable_text("Hello"));
        assert!(!s.is_translatable_text("https://example.com"));
        let options = ExtractionOptions::default();
        let rule = s.resolve("msg", None, &options);
        assert!(rule.text_translatable);
        assert!(!rule.ignored);
        assert!(!rule.attribute_translatable);
    }

    #[test]
    fn format_lookup() {
        // Compiled-in formats resolve case-insensitively; unknown names
        // list what is available.
        assert!(
            get_format("nope")
                .unwrap_err()
                .to_string()
                .contains("unsupported format")
        );
        for f in available_formats() {
            assert!(get_format(f).is_ok(), "{f}");
            assert!(get_format(&f.to_ascii_uppercase()).is_ok(), "{f}");
        }
    }

    #[test]
    fn available_formats_lists_all_builtins() {
        let formats = available_formats();
        let mut expected = Vec::new();
        if cfg!(feature = "mjml") {
            expected.push("mjml");
        }
        if cfg!(feature = "html") {
            expected.push("html");
        }
        if cfg!(feature = "xml") {
            expected.push("xml");
        }
        assert_eq!(formats, expected);
    }

    #[test]
    fn format_enum_names() {
        assert_eq!(Format::Mjml.name(), "mjml");
        assert_eq!(Format::Html.name(), "html");
        assert_eq!(Format::Xml.name(), "xml");
    }

    #[test]
    fn inline_classification() {
        for el in ["b", "i", "em", "strong", "span", "a", "br"] {
            assert!(HtmlStrategy.is_inline_element(el), "{el}");
        }
        for el in ["p", "div", "h1", "td", "button", "code", "pre"] {
            assert!(!HtmlStrategy.is_inline_element(el), "{el}");
        }
        assert!(MjmlStrategy.is_inline_element("strong"));
        assert!(MjmlStrategy.is_inline_element("em"));
        assert!(!MjmlStrategy.is_inline_element("mj-text"));
        // Custom strategies default to no inline folding.
        assert!(!Minimal.is_inline_element("msg"));
    }

    #[test]
    fn resolve_reports_inline_flag() {
        let options = ExtractionOptions::default();
        assert!(HtmlStrategy.resolve("b", None, &options).inline);
        assert!(!HtmlStrategy.resolve("p", None, &options).inline);
    }

    #[test]
    fn translate_directive_parsing() {
        let attrs = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect()
        };
        assert_eq!(
            translate_directive(&attrs(&[("translate", "no")])),
            Some(false)
        );
        assert_eq!(
            translate_directive(&attrs(&[("translate", "yes")])),
            Some(true)
        );
        assert_eq!(
            translate_directive(&attrs(&[("its:translate", "no")])),
            Some(false)
        );
        assert_eq!(
            translate_directive(&attrs(&[("TRANSLATE", "NO")])),
            Some(false)
        );
        assert_eq!(translate_directive(&attrs(&[("translate", "bogus")])), None);
        assert_eq!(translate_directive(&attrs(&[("class", "x")])), None);
    }

    #[test]
    fn effective_rule_respects_translate_attribute() {
        let options = ExtractionOptions::default();

        // translate="no" ignores the subtree, attributes included.
        let rule = effective_rule(&HtmlStrategy, "p", Some(false), &options);
        assert!(rule.ignored);
        assert!(!rule.text_translatable);

        // translate="yes" forces even unlisted elements.
        let rule = effective_rule(&HtmlStrategy, "figcaption", Some(true), &options);
        assert!(!rule.ignored);
        assert!(rule.text_translatable);

        // Ignored elements stay ignored even with translate="yes".
        let rule = effective_rule(&HtmlStrategy, "script", Some(true), &options);
        assert!(rule.ignored);

        // No directive: defaults.
        let rule = effective_rule(&HtmlStrategy, "p", None, &options);
        assert!(rule.text_translatable);

        // Opting out restores plain strategy behavior.
        let options = ExtractionOptions::builder()
            .respect_translate_attribute(false)
            .build();
        let rule = effective_rule(&HtmlStrategy, "p", Some(false), &options);
        assert!(rule.text_translatable);
    }
}
