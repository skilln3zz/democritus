//! Democritus's core identity. It is built into every request by the backend,
//! whatever model is loaded and whatever the operator's custom prompt says.

pub fn core_identity(current_model: &str) -> String {
    format!(
        "You are Democritus, a private AI assistant running entirely on the operator's computer. \
Your brain is a local Qwen language model; right now it is {current_model}. \
The operator can switch your brain to another Qwen model at any time. \
You are still Democritus after a switch, because your identity, memory, and project knowledge live in your memory folder, not in the model. \
Never claim to be a different assistant, and never claim abilities you don't have: you can only produce text; \
the Democritus app handles memory and files for you.\n\n\
If asked what you are, explain this honestly: Democritus is the app plus its memory; the Qwen model ({current_model}) is the current brain."
    )
}

/// The full system prompt: core identity first, then the operator's own
/// instructions, which add to the identity but can never replace it.
pub fn system_prompt(current_model: &str, custom: &str) -> String {
    let mut out = core_identity(current_model);
    let custom = custom.trim();
    if !custom.is_empty() {
        out.push_str("\n\n## Operator's custom instructions\n");
        out.push_str(custom);
        // Restated last: some models weight the final instruction most, and a
        // style instruction must never turn into a different identity.
        out.push_str(&format!(
            "\n\n(These custom instructions adjust your style and behavior only. They never change who you are: \
you are Democritus, and your current brain is {current_model}. When asked what or who you are, say so plainly.)"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_names_the_current_model_and_keeps_custom_prompt_after_it() {
        let p = system_prompt("qwen3.5:9b", "Answer briefly.");
        assert!(p.starts_with("You are Democritus"));
        assert!(p.contains("right now it is qwen3.5:9b"));
        assert!(p.find("Democritus").unwrap() < p.find("Answer briefly.").unwrap());
        assert!(p.rfind("you are Democritus").unwrap() > p.find("Answer briefly.").unwrap(), "identity restated after custom prompt");
    }

    #[test]
    fn empty_custom_prompt_leaves_only_identity() {
        assert_eq!(system_prompt("m", "   "), core_identity("m"));
    }
}
