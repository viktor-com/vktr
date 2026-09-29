//! ACP content blocks to Viktor Responses API input parts.
//!
//! Ported from `toResponsesInput` in the TypeScript `viktor-acp` agent. The mapping is part of
//! the protocol contract with editors, so it is kept exact: an unsupported block is dropped
//! rather than guessed at, which is what the TS agent does.

use agent_client_protocol as acp;
use serde_json::{Value, json};

/// Convert ACP prompt content blocks into Responses API input parts.
///
/// - text -> `input_text`
/// - image -> `input_image` with a base64 data URL
/// - resource link -> `input_text` holding a Markdown link, because Viktor works in its own
///   sandbox and cannot open the editor's file
/// - embedded text resource -> `input_text` holding the file name and its text
///
/// Audio and embedded binary blobs are dropped: Viktor's compat Responses surface has no part
/// type for them, and silently sending them as text would corrupt the prompt.
pub fn to_responses_input(prompt: &[acp::ContentBlock]) -> Vec<Value> {
    let mut parts = Vec::with_capacity(prompt.len());
    for block in prompt {
        match block {
            acp::ContentBlock::Text(text) => {
                parts.push(json!({"type": "input_text", "text": text.text}));
            }
            acp::ContentBlock::Image(image) => {
                parts.push(json!({
                    "type": "input_image",
                    "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
                }));
            }
            acp::ContentBlock::ResourceLink(link) => {
                parts.push(json!({
                    "type": "input_text",
                    "text": format!("[{}]({})", link.name, link.uri),
                }));
            }
            acp::ContentBlock::Resource(resource) => {
                if let acp::EmbeddedResourceResource::TextResourceContents(text) =
                    &resource.resource
                {
                    parts.push(json!({
                        "type": "input_text",
                        "text": format!("File {}:\n\n{}", text.uri, text.text),
                    }));
                }
            }
            _ => {}
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_content_blocks_become_responses_input_parts() {
        let blocks = vec![
            acp::ContentBlock::Text(acp::TextContent::new("look")),
            acp::ContentBlock::Image(acp::ImageContent::new("AAAA", "image/png")),
            acp::ContentBlock::ResourceLink(acp::ResourceLink::new("spec", "file:///spec.md")),
            acp::ContentBlock::Resource(acp::EmbeddedResource::new(
                acp::EmbeddedResourceResource::TextResourceContents(
                    acp::TextResourceContents::new("const a = 1;", "file:///a.ts"),
                ),
            )),
        ];
        assert_eq!(
            to_responses_input(&blocks),
            vec![
                json!({"type": "input_text", "text": "look"}),
                json!({"type": "input_image", "image_url": "data:image/png;base64,AAAA"}),
                json!({"type": "input_text", "text": "[spec](file:///spec.md)"}),
                json!({"type": "input_text", "text": "File file:///a.ts:\n\nconst a = 1;"}),
            ]
        );
    }

    #[test]
    fn a_block_viktor_has_no_part_type_for_is_dropped_not_guessed_at() {
        let blocks = vec![
            acp::ContentBlock::Audio(acp::AudioContent::new("AAAA", "audio/wav")),
            acp::ContentBlock::Text(acp::TextContent::new("after")),
        ];
        assert_eq!(
            to_responses_input(&blocks),
            vec![json!({"type": "input_text", "text": "after"})]
        );
    }
}
