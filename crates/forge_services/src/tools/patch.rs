use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use forge_display::{DiffFormat, TitleFormat};
use forge_domain::{
    ExecutableTool, FSPatchInput, NamedTool, PatchOperation, ToolCallContext, ToolDescription,
    ToolName, ToolOutput,
};
use forge_tool_macros::ToolDescription;
use thiserror::Error;
use tokio::fs;

use crate::services::EnvironmentService;
// No longer using dissimilar for fuzzy matching
use crate::tools::syn;
use crate::utils::{assert_absolute_path, format_display_path};
use crate::{FsWriteService, Infrastructure};

// Removed fuzzy matching threshold as we only use exact matching now

/// A match found in the source text. Represents a range in the source text that
/// can be used for extraction or replacement operations. Stores the position
/// and length to allow efficient substring operations.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
struct Range {
    /// Starting position of the match in source text
    start: usize,
    /// Length of the matched text
    length: usize,
}

impl Range {
    /// Create a new match from a start position and length
    fn new(start: usize, length: usize) -> Self {
        Self { start, length }
    }

    /// Get the end position (exclusive) of this match
    fn end(&self) -> usize {
        self.start + self.length
    }

    /// Try to find an exact match in the source text
    fn find_exact(source: &str, search: &str) -> Option<Self> {
        source
            .find(search)
            .map(|start| Self::new(start, search.len()))
    }

    // Fuzzy matching removed - we only use exact matching
}

impl From<Range> for std::ops::Range<usize> {
    fn from(m: Range) -> Self {
        m.start..m.end()
    }
}

// MatchSequence struct and implementation removed - we only use exact matching

#[derive(Debug, Error)]
enum Error {
    #[error("Failed to read/write file: {0}")]
    FileOperation(#[from] std::io::Error),
    #[error("Could not find match for search text: {0}")]
    NoMatch(String),
    #[error("Could not find swap target text: {0}")]
    NoSwapTarget(String),
}

fn apply_replacement(
    source: String,
    search: &str,
    operation: &PatchOperation,
    content: &str,
) -> Result<String, Error> {
    // Handle empty search string - only certain operations make sense here
    if search.is_empty() {
        return match operation {
            // Append to the end of the file
            PatchOperation::Append => Ok(format!("{source}{content}")),
            // Prepend to the beginning of the file
            PatchOperation::Prepend => Ok(format!("{content}{source}")),
            // Replace is equivalent to completely replacing the file
            PatchOperation::Replace => Ok(content.to_string()),
            // Swap doesn't make sense with empty search - keep source unchanged
            PatchOperation::Swap => Ok(source),
        };
    }

    // Find the exact match to operate on
    let patch =
        Range::find_exact(&source, search).ok_or_else(|| Error::NoMatch(search.to_string()))?;

    // Apply the operation based on its type
    match operation {
        // Prepend content before the matched text
        PatchOperation::Prepend => Ok(format!(
            "{}{}{}",
            &source[..patch.start],
            content,
            &source[patch.start..]
        )),

        // Append content after the matched text
        PatchOperation::Append => Ok(format!(
            "{}{}{}",
            &source[..patch.end()],
            content,
            &source[patch.end()..]
        )),

        // Replace matched text with new content
        PatchOperation::Replace => Ok(format!(
            "{}{}{}",
            &source[..patch.start],
            content,
            &source[patch.end()..]
        )),

        // Swap with another text in the source
        PatchOperation::Swap => {
            // Find the target text to swap with
            let target_patch = Range::find_exact(&source, content)
                .ok_or_else(|| Error::NoSwapTarget(content.to_string()))?;

            // Handle the case where patches overlap
            if (patch.start <= target_patch.start && patch.end() > target_patch.start)
                || (target_patch.start <= patch.start && target_patch.end() > patch.start)
            {
                // For overlapping ranges, we just do an ordinary replacement
                return Ok(format!(
                    "{}{}{}",
                    &source[..patch.start],
                    content,
                    &source[patch.end()..]
                ));
            }

            // We need to handle different ordering of patches
            if patch.start < target_patch.start {
                // Original text comes first
                Ok(format!(
                    "{}{}{}{}{}",
                    &source[..patch.start],
                    content,
                    &source[patch.end()..target_patch.start],
                    &source[patch.start..patch.end()],
                    &source[target_patch.end()..]
                ))
            } else {
                // Target text comes first
                Ok(format!(
                    "{}{}{}{}{}",
                    &source[..target_patch.start],
                    &source[patch.start..patch.end()],
                    &source[target_patch.end()..patch.start],
                    content,
                    &source[patch.end()..]
                ))
            }
        }
    }
}

// Using PatchOperation from forge_domain

// Using FSPatchInput from forge_domain

/// Modifies files with targeted text operations on matched patterns. Supports
/// prepend, append, replace, swap, delete operations on first pattern
/// occurrence. Ideal for precise changes to configs, code, or docs while
/// preserving context. Not suitable for complex refactoring or modifying all
/// pattern occurrences - use forge_tool_fs_create instead for complete
/// rewrites and forge_tool_fs_undo for undoing the last operation. Fails if
/// search pattern isn't found.
#[derive(ToolDescription)]
pub struct ApplyPatchJson<F>(Arc<F>);

impl<F: Infrastructure> NamedTool for ApplyPatchJson<F> {
    fn tool_name() -> ToolName {
        ToolName::new("forge_tool_fs_patch")
    }
}

impl<F: Infrastructure> ApplyPatchJson<F> {
    pub fn new(input: Arc<F>) -> Self {
        Self(input)
    }

    /// Formats a path for display, converting absolute paths to relative when
    /// possible
    ///
    /// If the path starts with the current working directory, returns a
    /// relative path. Otherwise, returns the original absolute path.
    fn format_display_path(&self, path: &Path) -> anyhow::Result<String> {
        // Get the current working directory
        let env = self.0.environment_service().get_environment();
        let cwd = env.cwd.as_path();

        // Use the shared utility function
        format_display_path(path, cwd)
    }
}

#[async_trait::async_trait]
impl<F: Infrastructure> ExecutableTool for ApplyPatchJson<F> {
    type Input = FSPatchInput;

    async fn call(
        &self,
        context: &mut ToolCallContext,
        patch: Self::Input,
    ) -> anyhow::Result<ToolOutput> {
        let path = Path::new(&patch.path);
        assert_absolute_path(path)?;

        // Read the original content once
        let mut current_content = fs::read_to_string(path)
            .await
            .map_err(Error::FileOperation)?;

        // Save the old content before modification for diff generation
        let old_content = current_content.clone();

        // Apply the replacement
        current_content = apply_replacement(
            current_content,
            &patch.search,
            &patch.operation,
            &patch.content,
        )?;

        // Format the display path for output
        let display_path = self.format_display_path(path)?;

        // Generate diff between old and new content
        let diff = DiffFormat::format(&old_content, &current_content);

        // Write final content to file after all patches are applied
        self.0
            .file_write_service()
            .write(path, Bytes::from(current_content.clone()))
            .await?;

        let mut result = String::new();

        writeln!(result, "---")?;
        writeln!(result, "path: {}", path.display())?;
        writeln!(result, "total_chars: {}", current_content.len())?;

        // Check for syntax errors
        if let Some(warning) = syn::validate(path, &current_content).map(|e| e.to_string()) {
            writeln!(result, "warning:{warning}")?;
        }

        writeln!(result, "---")?;

        writeln!(result, "{}", console::strip_ansi_codes(&diff).as_ref())?;

        context
            .send_text(format!(
                "{}",
                TitleFormat::debug("Patch").sub_title(display_path)
            ))
            .await?;

        // Output diff either to sender or println
        context.send_text(diff).await?;

        // Return the final result
        Ok(ToolOutput::text(result))
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::utils::TempDir;

    // Enhanced test helper for running multiple operations
    #[derive(Debug)]
    struct PatchTest {
        initial: String,
        patches: Vec<Patch>,
    }

    // Single operation with its result
    #[derive(Debug)]
    struct Patch {
        operation: PatchOperation,
        result: Result<String, String>,
    }

    // Represents a single patch operation
    #[derive(Debug)]
    struct PatchOperation {
        search: String,
        operation: forge_domain::PatchOperation,
        content: String,
    }

    // fmt::Display implementation removed in favor of using assert_debug_snapshot!

    impl PatchTest {
        fn new(initial: impl ToString) -> Self {
            PatchTest { initial: initial.to_string(), patches: Vec::new() }
        }

        /// Replace matched text with new content
        fn replace(mut self, search: impl ToString, content: impl ToString) -> Self {
            let operation = PatchOperation {
                search: search.to_string(),
                operation: forge_domain::PatchOperation::Replace,
                content: content.to_string(),
            };
            self.patches.push(Patch {
                operation,
                result: Err("Not executed yet".to_string()), // Placeholder
            });
            self
        }

        /// Prepend content before matched text
        fn prepend(mut self, search: impl ToString, content: impl ToString) -> Self {
            let operation = PatchOperation {
                search: search.to_string(),
                operation: forge_domain::PatchOperation::Prepend,
                content: content.to_string(),
            };
            self.patches.push(Patch {
                operation,
                result: Err("Not executed yet".to_string()), // Placeholder
            });
            self
        }

        /// Append content after matched text
        fn append(mut self, search: impl ToString, content: impl ToString) -> Self {
            let operation = PatchOperation {
                search: search.to_string(),
                operation: forge_domain::PatchOperation::Append,
                content: content.to_string(),
            };
            self.patches.push(Patch {
                operation,
                result: Err("Not executed yet".to_string()), // Placeholder
            });
            self
        }

        /// Swap matched text with target text
        fn swap(mut self, search: impl ToString, target: impl ToString) -> Self {
            let operation = PatchOperation {
                search: search.to_string(),
                operation: forge_domain::PatchOperation::Swap,
                content: target.to_string(),
            };
            self.patches.push(Patch {
                operation,
                result: Err("Not executed yet".to_string()), // Placeholder
            });
            self
        }

        /// Try to execute all operations and record their results
        fn execute_all(mut self) -> Self {
            let mut current_content = self.initial.clone();

            for op_result in &mut self.patches {
                // Apply the operation
                let result = match apply_replacement(
                    current_content.clone(),
                    &op_result.operation.search,
                    &op_result.operation.operation,
                    &op_result.operation.content,
                ) {
                    Ok(content) => {
                        // Update the current content for the next operation
                        current_content = content.clone();
                        Ok(content)
                    }
                    Err(err) => Err(err.to_string()),
                };

                // Update the result
                op_result.result = result;
            }

            self
        }
    }

    #[test]
    fn comprehensive_patch_tests() {
        // Create a comprehensive test that includes all the test cases
        let test = PatchTest::new("Hello World")
            // Basic Operations
            .replace("World", "Forge")
            .replace("", " bar")
            // Single Replacement Behavior
            .replace("foo", "baz")
            // Exact Matching Behavior
            .replace("Hello", "Hi")
            // Unicode and Special Characters
            .replace("Hello", "你好")
            .replace("World", "🌍")
            // Whitespace Handling
            .prepend("Hello", "    ")
            .append("World", "\n  New line")
            // Test different operation types
            .prepend("Hello", "Greetings, ")
            .append("World", "!")
            .swap("Hello", "World")
            // Empty search operations
            .prepend("", "Start: ")
            .append("", " End")
            .replace("", "Completely New Content")
            // Execute all operations and collect results
            .execute_all();

        // Snapshot the entire test result using Debug representation
        insta::assert_debug_snapshot!(test);
    }

    #[test]
    fn comprehensive_error_tests() {
        // Create a test specifically for error cases
        let test = PatchTest::new("foo bar baz")
            .replace("nonexistent", "replaced")
            .replace("foo-bar", "replaced")
            .replace("afoo", "replaced")
            .swap("foo", "nonexistent")
            .execute_all();

        // Snapshot the error test results using Debug representation
        insta::assert_debug_snapshot!(test);
    }

    // The previous individual tests are removed since they're now consolidated

    #[tokio::test]
    async fn test_format_display_path() {
        use std::sync::Arc;

        use crate::attachment::tests::MockInfrastructure;

        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create a mock infrastructure with controlled cwd
        let infra = Arc::new(MockInfrastructure::new());
        let patch_tool = ApplyPatchJson::new(infra);

        // Test with a mock path
        let display_path = patch_tool.format_display_path(Path::new(&file_path));

        // Since MockInfrastructure has a fixed cwd of "/test",
        // and our temp path won't start with that, we expect the full path
        assert!(display_path.is_ok());
        assert_eq!(display_path.unwrap(), file_path.display().to_string());
    }
}
