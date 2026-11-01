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

    /// Whether a text node is translatable. Defaults to the shared
    /// heuristic in [`crate::filter`].
    fn is_translatable_text(&self, text: &str) -> bool {
        filter::is_translatable_text(text)
    }

    /// Resolves the effective element set by merging the strategy rules
    /// with user-supplied [`ExtractionOptions`].
    ///
    /// The returned flags are, in order: text translatable, element
    /// ignored, attribute translatable.
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
        }
    }
}

/// Outcome of merging a strategy with user options for one element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRule {
    /// Text content of the element (or the attribute value) is translatable.
    pub text_translatable: bool,
    /// Element is skipped entirely.
    pub ignored: bool,
    /// The attribute (if any) is translatable.
    pub attribute_translatable: bool,
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
        assert!(get_format("mjml").is_ok());
        assert!(get_format("MJML").is_ok());
        assert!(get_format("Html").is_ok());
        assert!(get_format("xml").is_ok());
        assert!(get_strategy("XML").is_ok());
        let err = get_format("nope").unwrap_err();
        assert!(err.to_string().contains("unsupported format"));
        for f in available_formats() {
            assert!(get_format(f).is_ok(), "{f}");
        }
    }

    #[test]
    fn available_formats_lists_all_builtins() {
        let formats = available_formats();
        assert!(formats.contains(&"mjml"));
        assert!(formats.contains(&"html"));
        assert!(formats.contains(&"xml"));
        assert_eq!(formats.len(), 3);
    }

    #[test]
    fn format_enum_names() {
        assert_eq!(Format::Mjml.name(), "mjml");
        assert_eq!(Format::Html.name(), "html");
        assert_eq!(Format::Xml.name(), "xml");
    }
}
