use derive_setters::Setters;
use serde::{Deserialize, Serialize};

use crate::{Image, ToolCallFull, ToolCallId, ToolName};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Setters)]
#[setters(into)]
pub struct ToolResult {
    pub name: ToolName,
    pub call_id: Option<ToolCallId>,
    #[setters(skip)]
    pub output: ToolOutput,
}

impl ToolResult {
    pub fn new(name: ToolName) -> ToolResult {
        Self {
            name,
            call_id: Default::default(),
            output: Default::default(),
        }
    }

    pub fn success(mut self, content: impl Into<String>) -> Self {
        self.output = ToolOutput::text(content.into());

        self
    }

    pub fn failure(self, err: anyhow::Error) -> Self {
        self.output(Err(err))
    }

    pub fn is_error(&self) -> bool {
        self.output.is_error
    }

    pub fn output(mut self, result: Result<ToolOutput, anyhow::Error>) -> Self {
        match result {
            Ok(output) => {
                self.output = output;
            }
            Err(err) => {
                self.output = ToolOutput::text(format!("{err:?}")).is_error(true);
            }
        }
        self
    }
}

impl From<ToolCallFull> for ToolResult {
    fn from(value: ToolCallFull) -> Self {
        Self {
            name: value.name,
            call_id: value.call_id,
            output: Default::default(),
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Setters)]
#[setters(into, strip_option)]
pub struct ToolOutput {
    pub values: Vec<ToolOutputValue>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(tool: String) -> Self {
        ToolOutput {
            is_error: Default::default(),
            values: vec![ToolOutputValue::Text(tool)],
        }
    }

    pub fn image(img: Image) -> Self {
        ToolOutput { is_error: false, values: vec![ToolOutputValue::Image(img)] }
    }

    pub fn combine(self, other: ToolOutput) -> Self {
        let mut items = self.values;
        items.extend(other.values);
        ToolOutput { values: items, is_error: self.is_error || other.is_error }
    }

    /// Returns the first item as a string if it exists
    pub fn as_str(&self) -> Option<&str> {
        self.values.iter().find_map(|item| item.as_str())
    }
}

impl<T> From<T> for ToolOutput
where
    T: Iterator<Item = ToolOutput>,
{
    fn from(item: T) -> Self {
        item.fold(ToolOutput::default(), |acc, item| acc.combine(item))
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub enum ToolOutputValue {
    Text(String),
    Image(Image),
    #[default]
    Empty,
}

impl ToolOutputValue {
    pub fn text(text: String) -> Self {
        ToolOutputValue::Text(text)
    }

    pub fn image(img: Image) -> Self {
        ToolOutputValue::Image(img)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            ToolOutputValue::Text(text) => Some(text),
            ToolOutputValue::Image(_) => None,
            ToolOutputValue::Empty => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_success_and_failure_content() {
        let success = ToolResult::new(ToolName::new("test_tool")).success("success message");
        assert!(!success.is_error());
        assert_eq!(success.output.as_str().unwrap(), "success message");

        let failure =
            ToolResult::new(ToolName::new("test_tool")).failure(anyhow::anyhow!("error message"));
        assert!(failure.is_error());
        // The actual error format from anyhow::Error is "Error: error message"
        // Don't test the exact string format as it might change
        let error_message = failure.output.as_str().unwrap();
        assert!(error_message.contains("error message"));
    }
}
