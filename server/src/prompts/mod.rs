use rmcp::model::*;

pub fn list_prompts() -> Vec<Prompt> {
    vec![
        Prompt {
            name: "support_response".into(),
            title: Some("Generate Support Response".into()),
            description: Some("Generate a customer support response for a ticket".into()),
            arguments: Some(vec![
                PromptArgument {
                    name: "ticket_id".into(),
                    title: Some("Ticket ID".into()),
                    description: Some("The ticket ID to respond to".into()),
                    required: Some(true),
                },
                PromptArgument {
                    name: "tone".into(),
                    title: Some("Tone".into()),
                    description: Some("Response tone: empathetic, professional, casual".into()),
                    required: Some(false),
                },
            ]),
            icons: None,
            meta: None,
        },
        Prompt {
            name: "survey_creation".into(),
            title: Some("Create Survey".into()),
            description: Some("Help create a customer survey".into()),
            arguments: Some(vec![
                PromptArgument {
                    name: "topic".into(),
                    title: Some("Topic".into()),
                    description: Some("Survey topic or goal".into()),
                    required: Some(true),
                },
                PromptArgument {
                    name: "question_count".into(),
                    title: Some("Question Count".into()),
                    description: Some("Number of questions to generate".into()),
                    required: Some(false),
                },
            ]),
            icons: None,
            meta: None,
        },
        Prompt {
            name: "customer_summary".into(),
            title: Some("Customer Summary".into()),
            description: Some("Summarize a customer's history and engagement".into()),
            arguments: Some(vec![PromptArgument {
                name: "customer_id".into(),
                title: Some("Customer ID".into()),
                description: Some("The customer ID to summarize".into()),
                required: Some(true),
            }]),
            icons: None,
            meta: None,
        },
        Prompt {
            name: "escalation".into(),
            title: Some("Escalation Report".into()),
            description: Some("Generate an escalation report for a ticket".into()),
            arguments: Some(vec![
                PromptArgument {
                    name: "ticket_id".into(),
                    title: Some("Ticket ID".into()),
                    description: Some("The ticket ID to escalate".into()),
                    required: Some(true),
                },
                PromptArgument {
                    name: "reason".into(),
                    title: Some("Reason".into()),
                    description: Some("Reason for escalation".into()),
                    required: Some(true),
                },
            ]),
            icons: None,
            meta: None,
        },
    ]
}

pub fn get_prompt(name: &str, args: &serde_json::Value) -> Option<GetPromptResult> {
    match name {
        "support_response" => {
            let ticket_id = args.get("ticket_id")?.as_str()?;
            let tone = args
                .get("tone")
                .and_then(|v| v.as_str())
                .unwrap_or("professional");
            Some(GetPromptResult {
                description: Some("Generate a support response".into()),
                messages: vec![PromptMessage {
                    role: PromptMessageRole::User,
                    content: PromptMessageContent::Text {
                        text: format!(
                            "You are a customer support agent. Look up ticket {ticket_id} using the get_ticket tool, \
                             review the customer's history with get_customer_360, and draft a {tone} response \
                             that addresses their concern. If relevant, search the knowledge base for helpful articles."
                        ),
                    },
                }],
            })
        }
        "survey_creation" => {
            let topic = args.get("topic")?.as_str()?;
            let count = args
                .get("question_count")
                .and_then(|v| v.as_str())
                .unwrap_or("5");
            Some(GetPromptResult {
                description: Some("Create a customer survey".into()),
                messages: vec![PromptMessage {
                    role: PromptMessageRole::User,
                    content: PromptMessageContent::Text {
                        text: format!(
                            "Create a customer survey about '{topic}' with {count} questions. \
                             Include a mix of rating scales (1-5), multiple choice, and open-ended questions. \
                             Format the questions as JSON suitable for the create_survey tool."
                        ),
                    },
                }],
            })
        }
        "customer_summary" => {
            let customer_id = args.get("customer_id")?.as_str()?;
            Some(GetPromptResult {
                description: Some("Summarize customer engagement".into()),
                messages: vec![PromptMessage {
                    role: PromptMessageRole::User,
                    content: PromptMessageContent::Text {
                        text: format!(
                            "Use get_customer_360 to retrieve the full profile for customer {customer_id}. \
                             Summarize their engagement history, support interactions, feedback sentiment, \
                             and provide actionable insights for the account team."
                        ),
                    },
                }],
            })
        }
        "escalation" => {
            let ticket_id = args.get("ticket_id")?.as_str()?;
            let reason = args.get("reason")?.as_str()?;
            Some(GetPromptResult {
                description: Some("Generate escalation report".into()),
                messages: vec![PromptMessage {
                    role: PromptMessageRole::User,
                    content: PromptMessageContent::Text {
                        text: format!(
                            "Generate an escalation report for ticket {ticket_id}. Reason: {reason}. \
                             Look up the ticket details, customer history, and any related feedback. \
                             Include: summary, impact assessment, recommended actions, and priority level."
                        ),
                    },
                }],
            })
        }
        _ => None,
    }
}
