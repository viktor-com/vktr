//! Tool-result images for the Viktor compat API (Responses backend).
//!
//! Viktor keeps only the text of a `function_call_output` content list and drops its `input_image`
//! parts, so an image a tool returned (read_file on a PNG) never reached the model, which then tried to
//! pull the bytes through the shell as base64 text. Images in a user message do reach the model, so for
//! Viktor each run of consecutive tool results keeps its text and hands its images over in one user
//! message placed right after the run.

use async_openai::types::responses as rs;

use crate::viktor_continuation::VIKTOR_MODEL;

const IMAGES_NOTE: &str = "Images returned by the tool results above.";

/// Rewrite `request.input` in place for Viktor; any other model is left untouched.
pub(crate) fn move_to_user_message(effective_model: &str, request: &mut rs::CreateResponse) {
    if effective_model != VIKTOR_MODEL {
        return;
    }
    let rs::InputParam::Items(items) = &mut request.input else {
        return;
    };
    if !items.iter().any(output_has_image) {
        return;
    }
    let mut out = Vec::with_capacity(items.len() + 1);
    let mut images: Vec<rs::InputContent> = Vec::new();
    for mut item in std::mem::take(items) {
        let is_output = matches!(&item, rs::InputItem::Item(rs::Item::FunctionCallOutput(_)));
        if !is_output && !images.is_empty() {
            out.push(user_message(std::mem::take(&mut images)));
        }
        if let rs::InputItem::Item(rs::Item::FunctionCallOutput(output)) = &mut item {
            take_images(&mut output.output, &mut images);
        }
        out.push(item);
    }
    if !images.is_empty() {
        out.push(user_message(images));
    }
    *items = out;
}

fn output_has_image(item: &rs::InputItem) -> bool {
    matches!(
        item,
        rs::InputItem::Item(rs::Item::FunctionCallOutput(rs::FunctionCallOutputItemParam {
            output: rs::FunctionCallOutput::Content(parts),
            ..
        })) if parts.iter().any(|p| matches!(p, rs::InputContent::InputImage(_)))
    )
}

/// Move the image parts of `output` into `images`, leaving the output as plain text.
fn take_images(output: &mut rs::FunctionCallOutput, images: &mut Vec<rs::InputContent>) {
    let rs::FunctionCallOutput::Content(parts) = output else {
        return;
    };
    let mut text = String::new();
    for part in std::mem::take(parts) {
        match part {
            rs::InputContent::InputText(t) => text.push_str(&t.text),
            image @ rs::InputContent::InputImage(_) => images.push(image),
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        *output = rs::FunctionCallOutput::Text(text);
    } else {
        parts.insert(
            0,
            rs::InputContent::InputText(rs::InputTextContent { text }),
        );
    }
}

fn user_message(images: Vec<rs::InputContent>) -> rs::InputItem {
    let mut content = vec![rs::InputContent::InputText(rs::InputTextContent {
        text: IMAGES_NOTE.to_owned(),
    })];
    content.extend(images);
    rs::InputItem::EasyMessage(rs::EasyInputMessage {
        r#type: rs::MessageType::Message,
        role: rs::Role::User,
        content: rs::EasyInputContent::ContentList(content),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(input: serde_json::Value) -> rs::CreateResponse {
        serde_json::from_value(json!({"model": "viktor", "input": input})).unwrap()
    }

    fn input_json(request: &rs::CreateResponse) -> serde_json::Value {
        serde_json::to_value(&request.input).unwrap()
    }

    const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

    fn image_output(call_id: &str) -> serde_json::Value {
        json!({"type": "function_call_output", "call_id": call_id, "output": [
            {"type": "input_text", "text": "Read image file: /tmp/shot.png"},
            {"type": "input_image", "image_url": PNG, "detail": "auto"},
        ]})
    }

    #[test]
    fn a_tool_result_image_reaches_viktor_in_a_user_message() {
        let mut req = request(json!([
            {"type": "function_call", "call_id": "a", "name": "read_file", "arguments": "{}"},
            image_output("a"),
        ]));
        move_to_user_message("viktor", &mut req);
        let input = input_json(&req);
        assert_eq!(input[1]["output"], "Read image file: /tmp/shot.png");
        assert_eq!(input[2]["role"], "user");
        assert_eq!(input[2]["content"][0]["text"], IMAGES_NOTE);
        assert_eq!(input[2]["content"][1]["type"], "input_image");
        assert_eq!(input[2]["content"][1]["image_url"], PNG);
        assert_eq!(input.as_array().unwrap().len(), 3);
        assert!(!input[1].to_string().contains("base64"));
    }

    #[test]
    fn parallel_tool_results_stay_together_before_the_images() {
        let mut req = request(json!([
            image_output("a"),
            {"type": "function_call_output", "call_id": "b", "output": "plain"},
            image_output("c"),
            {"type": "message", "role": "user", "content": "next"},
        ]));
        move_to_user_message("viktor", &mut req);
        let input = input_json(&req);
        let kinds: Vec<_> = input
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                i["call_id"]
                    .as_str()
                    .or(i["role"].as_str())
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(kinds, ["a", "b", "c", "user", "user"]);
        assert_eq!(input[3]["content"].as_array().unwrap().len(), 3);
        assert_eq!(input[4]["content"], "next");
    }

    #[test]
    fn other_models_and_text_only_results_are_untouched() {
        let original = json!([image_output("a")]);
        let mut req = request(original.clone());
        move_to_user_message("grok-4", &mut req);
        assert_eq!(input_json(&req), input_json(&request(original)));

        let text_only = json!([
            {"type": "function_call_output", "call_id": "a", "output": "x"},
            {"type": "function_call_output", "call_id": "b", "output": [{"type": "input_text", "text": "y"}]},
        ]);
        let mut req = request(text_only.clone());
        move_to_user_message("viktor", &mut req);
        assert_eq!(input_json(&req), input_json(&request(text_only)));
    }
}
