use std::path::Path;
use std::sync::Arc;

use forge_display::TitleFormat;
use forge_domain::{
    ExecutableTool, NamedTool, ToolCallContext, ToolDescription, ToolName, ToolOutput, UndoInput,
};
use forge_tool_macros::ToolDescription;

use crate::infra::FsSnapshotService;
use crate::services::EnvironmentService;
use crate::utils::{assert_absolute_path, format_display_path};
use crate::Infrastructure;

/// Reverts the most recent file operation (create/modify/delete) on a specific
/// file. Use this tool when you need to recover from incorrect file changes or
/// if a revert is requested by the user.
#[derive(Default, ToolDescription)]
pub struct FsUndo<F>(Arc<F>);

impl<F> FsUndo<F> {
    pub fn new(infra: Arc<F>) -> Self {
        Self(infra)
    }
}

impl<F: Infrastructure> FsUndo<F> {
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

impl<F> NamedTool for FsUndo<F> {
    fn tool_name() -> ToolName {
        ToolName::new("forge_tool_fs_undo")
    }
}

#[async_trait::async_trait]
impl<F: Infrastructure> ExecutableTool for FsUndo<F> {
    type Input = UndoInput;
    async fn call(
        &self,
        context: &mut ToolCallContext,
        input: Self::Input,
    ) -> anyhow::Result<ToolOutput> {
        let path = Path::new(&input.path);
        assert_absolute_path(path)?;

        self.0.file_snapshot_service().undo_snapshot(path).await?;

        // Format the path for display
        let display_path = self.format_display_path(path)?;

        // Display a message about the file being undone
        let message = TitleFormat::debug("Undo").sub_title(display_path.clone());
        context.send_text(message).await?;

        Ok(ToolOutput::text(format!(
            "Successfully undid last operation on path: {display_path}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::*;
    use crate::attachment::tests::MockInfrastructure;
    use crate::tools::registry::tests::Stub;

    #[tokio::test]
    async fn test_successful_undo() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("success.txt");
        let infra = Arc::new(Stub::default());
        let undo = FsUndo::new(infra);

        // Act
        let result = undo
            .call(
                &mut ToolCallContext::default(),
                UndoInput {
                    path: test_path.to_string_lossy().to_string(),
                    explanation: None,
                },
            )
            .await;

        // Assert
        assert!(result.is_ok(), "Expected successful undo operation");
        assert_eq!(
            result.unwrap(),
            ToolOutput::text(format!(
                "Successfully undid last operation on path: {}",
                test_path.display()
            ),),
            "Unexpected success message"
        );
    }

    #[tokio::test]
    async fn test_tool_name() {
        assert_eq!(
            FsUndo::<Stub>::tool_name().to_string(),
            "forge_tool_fs_undo",
            "Tool name should match expected value"
        );
    }

    #[tokio::test]
    async fn test_format_display_path() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create a mock infrastructure with controlled cwd
        let infra = Arc::new(MockInfrastructure::new());
        let fs_undo = FsUndo::new(infra);

        // Test with a mock path
        let display_path = fs_undo.format_display_path(Path::new(&file_path));

        // Since MockInfrastructure has a fixed cwd of "/test",
        // and our temp path won't start with that, we expect the full path
        assert!(display_path.is_ok());
        assert_eq!(display_path.unwrap(), file_path.display().to_string());
    }
}
