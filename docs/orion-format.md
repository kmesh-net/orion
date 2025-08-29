# Envoy Access Log Formatter

Orion-format is a Rust crate designed to format Envoy access logs efficiently. It provides a way to construct log formatters that can be cloned and used to format access log entries without incurring significant performance overhead.

The formatter leverages several micro-optimizations to significantly accelerate the process.

## Design Principles
- Blueprinting: Once constructed with a given format string, the formatter is intended to be cheaply cloned for each log instance. Cloning only duplicates a Vec<Option<SmolStr>>, resulting in a single dynamic memory allocation.
- Clone-less Evaluation: Operators are evaluated by referencing context objects which provide the necessary information for rendering the log message. Several predefined contexts are supported, such as DownstreamRequest, UpstreamRequest, DownstreamResponse, UpstreamResponse, as well as general-purpose contexts like InitContext and FinishContext containing metadata not available from the HTTP layer.
- No Dynamic Memory Allocation: Formatting occurs within a pre-allocated vector of SmolStr. Most Envoy operators produce outputs under 23 characters, so evaluation of an operator typically avoids heap allocation altogether.
- Validation of Envoy Access Log Grammar: The grammar is validated during formatter construction.
- Full Support for the Default Envoy Access Log Format: This includes compatibility with Istio’s access log format, which is identical.

## Limitations
  - The entire Envoy grammar is supported except for the ? operator, which is currently only accepted for the X-ENVOY-ORIGINAL-PATH header.
  - Only a subset of Envoy operators is implemented. If an unsupported operator is encountered, LogFormatter construction will fail, causing bootstrap/XDS configuration parsing to fail as well.
  - `omit_empty_values` is not supported. If an operator evaluates to an empty string, it will be rendered as a '-'.
  - The crate does not support json formatting.

## Additional Information
  - The crate includes unit and benchmark tests (via criterion). Performance is strong: cloning a LogFormatter and formatting the default Envoy log message is approximately 60% faster than cloning metadata and formatting with the Rust format!() macro.
  - The crate also defines additional types such as ResponseFlags, which can be logged in both short and long format.
