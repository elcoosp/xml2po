// examples/wasm-usage.js
// Example usage of the WASM library in JavaScript/TypeScript

import init, {
  extract_from_content,
  extract_to_po,
  compile_with_po,
  get_supported_formats,
} from "./pkg/xml_po_converter.js";

async function main() {
  // Initialize WASM module
  await init();

  // Get supported formats
  const formats = get_supported_formats();
  console.log("Supported formats:", formats);

  // Example MJML content
  const mjmlContent = `
    <mjml>
      <mj-body>
        <mj-section>
          <mj-column>
            <mj-text>Hello World</mj-text>
            <mj-button>Click here</mj-button>
          </mj-column>
        </mj-section>
      </mj-body>
    </mjml>
  `;

  // Example HTML content
  const htmlContent = `
    <!DOCTYPE html>
    <html>
      <head>
        <title>Welcome</title>
      </head>
      <body>
        <h1>Hello World</h1>
        <p>This is a paragraph</p>
        <button>Click me</button>
      </body>
    </html>
  `;

  try {
    // Extract messages from MJML
    console.log("\n--- Extracting from MJML ---");
    const mjmlMessages = extract_from_content(
      mjmlContent,
      "mjml",
      "example.mjml",
    );
    console.log("Extracted messages:", mjmlMessages);

    // Extract to PO format
    const poContent = extract_to_po(mjmlContent, "mjml", "example.mjml");
    console.log("\nPO file content:\n", poContent);

    // Example PO file with translations
    const poWithTranslations = `
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\\n"

#: example.mjml:123
msgid "Hello World"
msgstr "Bonjour le monde"

#: example.mjml:456
msgid "Click here"
msgstr "Cliquez ici"
    `;

    // Compile with translations
    console.log("\n--- Compiling with translations ---");
    const translated = compile_with_po(mjmlContent, poWithTranslations, "mjml");
    console.log("Translated content:\n", translated);

    // Extract from HTML
    console.log("\n--- Extracting from HTML ---");
    const htmlMessages = extract_from_content(
      htmlContent,
      "html",
      "example.html",
    );
    console.log("Extracted messages:", htmlMessages);
  } catch (error) {
    console.error("Error:", error);
  }
}

// Run if in Node.js environment
if (typeof window === "undefined") {
  main();
}

// Export for browser usage
export {
  init,
  extract_from_content,
  extract_to_po,
  compile_with_po,
  get_supported_formats,
};

// ===== React/Vue Example =====

// Example React component
export function TranslationExtractor() {
  const [content, setContent] = React.useState("");
  const [format, setFormat] = React.useState("mjml");
  const [poOutput, setPoOutput] = React.useState("");
  const [error, setError] = React.useState("");

  const handleExtract = async () => {
    try {
      await init();
      const po = extract_to_po(content, format, "input");
      setPoOutput(po);
      setError("");
    } catch (err) {
      setError(err.toString());
    }
  };

  return (
    <div className="extractor">
      <h2>Translation Extractor</h2>

      <div>
        <label>Format:</label>
        <select value={format} onChange={(e) => setFormat(e.target.value)}>
          <option value="mjml">MJML</option>
          <option value="html">HTML</option>
        </select>
      </div>

      <div>
        <label>Content:</label>
        <textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          rows={10}
          cols={50}
          placeholder="Paste your MJML or HTML here..."
        />
      </div>

      <button onClick={handleExtract}>Extract Translations</button>

      {error && <div className="error">{error}</div>}

      {poOutput && (
        <div>
          <h3>PO Output:</h3>
          <pre>{poOutput}</pre>
        </div>
      )}
    </div>
  );
}

// Example Vue component
export const VueTranslationExtractor = {
  template: `
    <div class="extractor">
      <h2>Translation Extractor</h2>

      <div>
        <label>Format:</label>
        <select v-model="format">
          <option value="mjml">MJML</option>
          <option value="html">HTML</option>
        </select>
      </div>

      <div>
        <label>Content:</label>
        <textarea
          v-model="content"
          rows="10"
          cols="50"
          placeholder="Paste your MJML or HTML here..."
        />
      </div>

      <button @click="handleExtract">Extract Translations</button>

      <div v-if="error" class="error">{{ error }}</div>

      <div v-if="poOutput">
        <h3>PO Output:</h3>
        <pre>{{ poOutput }}</pre>
      </div>
    </div>
  `,
  data() {
    return {
      content: "",
      format: "mjml",
      poOutput: "",
      error: "",
      initialized: false,
    };
  },
  methods: {
    async handleExtract() {
      try {
        if (!this.initialized) {
          await init();
          this.initialized = true;
        }
        this.poOutput = extract_to_po(this.content, this.format, "input");
        this.error = "";
      } catch (err) {
        this.error = err.toString();
      }
    },
  },
};
