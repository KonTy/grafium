//! Diagnostic: what does Chat actually do with a given question?
//!
//! Prints the routing decision (research trigger, general-knowledge override)
//! for each argument, so a phrasing that "should have worked" can be checked
//! without driving the UI.
//!
//! Usage:
//!   cargo run -p grafium-core --release --example intent_check -- "question one" "question two"
use grafium_core::knowledge::research_intent::{detect_research_intent, wants_general_knowledge};

fn main() {
    let questions: Vec<String> = std::env::args().skip(1).collect();
    for q in &questions {
        let research = detect_research_intent(q);
        let general = wants_general_knowledge(q);
        println!(
            "{:<62} research={:<5} general={}",
            truncate(q, 60),
            research.is_some(),
            general
        );
        if let Some(r) = research {
            println!(
                "{:>16}cleaned: {:?}  needs_context={}",
                "", r.cleaned_question, r.needs_conversation_context
            );
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}
