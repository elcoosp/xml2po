//! User-tunable extraction configuration.

/// Runtime knobs layered on top of an [`ExtractionStrategy`](crate::ExtractionStrategy).
///
/// Build one with the fluent [`ExtractionOptions::builder`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "serde", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize),
    serde(default)
)]
pub struct ExtractionOptions {
    /// Extra elements whose text content should be extracted even when the
    /// strategy does not list them.
    pub extra_translatable_elements: Vec<String>,
    /// Elements to skip entirely, overriding strategy defaults.
    pub ignored_elements: Vec<String>,
    /// Extra attribute names whose values should be extracted.
    pub translatable_attributes: Vec<String>,
    /// Emit gettext contexts (`msgctxt`): the element name for text
    /// content, `element@attribute` for attribute values.
    pub with_context: bool,
    /// Fold inline elements (`<b>`, `<em>`, `<span>`, …) into the
    /// surrounding message as `<tag>` placeholders instead of splitting
    /// mixed content into separate strings. Defaults to `true`.
    pub inline_placeholders: bool,
    /// Honor the HTML5/ITS `translate="yes|no"` attribute (and
    /// `its:translate`): `no` skips the subtree, `yes` forces extraction.
    /// Defaults to `true`.
    pub respect_translate_attribute: bool,
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self {
            extra_translatable_elements: Vec::new(),
            ignored_elements: Vec::new(),
            translatable_attributes: Vec::new(),
            with_context: false,
            inline_placeholders: true,
            respect_translate_attribute: true,
        }
    }
}

impl ExtractionOptions {
    /// Returns a builder for these options.
    #[must_use]
    pub fn builder() -> ExtractionOptionsBuilder {
        ExtractionOptionsBuilder::default()
    }
}

/// Fluent builder for [`ExtractionOptions`].
#[derive(Debug, Clone, Default)]
pub struct ExtractionOptionsBuilder {
    options: ExtractionOptions,
}

impl ExtractionOptionsBuilder {
    /// Adds an element whose text is always extracted.
    #[must_use]
    pub fn extra_translatable_element(mut self, element: impl Into<String>) -> Self {
        self.options
            .extra_translatable_elements
            .push(element.into());
        self
    }

    /// Adds an element that is skipped entirely.
    #[must_use]
    pub fn ignored_element(mut self, element: impl Into<String>) -> Self {
        self.options.ignored_elements.push(element.into());
        self
    }

    /// Adds a translatable attribute name.
    #[must_use]
    pub fn translatable_attribute(mut self, attribute: impl Into<String>) -> Self {
        self.options.translatable_attributes.push(attribute.into());
        self
    }

    /// Enables gettext context emission (`msgctxt`).
    #[must_use]
    pub const fn with_context(mut self, with: bool) -> Self {
        self.options.with_context = with;
        self
    }

    /// Toggles inline placeholder folding (default: on).
    #[must_use]
    pub const fn inline_placeholders(mut self, inline: bool) -> Self {
        self.options.inline_placeholders = inline;
        self
    }

    /// Toggles honoring the `translate="yes|no"` attribute (default: on).
    #[must_use]
    pub const fn respect_translate_attribute(mut self, respect: bool) -> Self {
        self.options.respect_translate_attribute = respect;
        self
    }

    /// Finalizes the options.
    #[must_use]
    pub fn build(self) -> ExtractionOptions {
        self.options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_accumulates_and_defaults() {
        let default = ExtractionOptions::default();
        assert_eq!(
            default,
            ExtractionOptions {
                extra_translatable_elements: Vec::new(),
                ignored_elements: Vec::new(),
                translatable_attributes: Vec::new(),
                with_context: false,
                inline_placeholders: true,
                respect_translate_attribute: true,
            }
        );

        let built = ExtractionOptions::builder()
            .extra_translatable_element("a")
            .extra_translatable_element("b")
            .ignored_element("c")
            .translatable_attribute("data-x")
            .with_context(true)
            .build();
        assert_eq!(
            built.extra_translatable_elements,
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(built.ignored_elements, vec!["c".to_string()]);
        assert_eq!(built.translatable_attributes, vec!["data-x".to_string()]);
        assert!(built.with_context);
    }

    #[test]
    fn builder_can_toggle_context_off() {
        let built = ExtractionOptions::builder()
            .with_context(true)
            .with_context(false)
            .build();
        assert!(!built.with_context);
    }

    #[test]
    fn new_fields_default_to_enabled() {
        let default = ExtractionOptions::default();
        assert!(default.inline_placeholders);
        assert!(default.respect_translate_attribute);
        let built = ExtractionOptions::builder().build();
        assert_eq!(built, default);
    }

    #[test]
    fn new_fields_can_be_disabled_via_builder() {
        let built = ExtractionOptions::builder()
            .inline_placeholders(false)
            .respect_translate_attribute(false)
            .build();
        assert!(!built.inline_placeholders);
        assert!(!built.respect_translate_attribute);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serde_roundtrip_keeps_true_defaults() {
        let json = r"{}";
        let parsed: ExtractionOptions = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, ExtractionOptions::default());

        let options = ExtractionOptions::builder()
            .inline_placeholders(false)
            .build();
        let json = serde_json::to_string(&options).unwrap();
        let parsed: ExtractionOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, options);
    }
}
