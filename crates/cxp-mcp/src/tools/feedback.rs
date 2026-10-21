//! Feedback and surveys, and the analytics over them. The three billable tools
//! of this service live here: `send_survey`, `submit_feedback`, and
//! `submit_survey_response` each charge one usage event on their value meter
//! in their own transaction, before their write, so a failed call is never
//! billed. No other tool charges; `create_survey` is free.

use cxp_core::analysis::sentiment::{analyze_sentiment, SentimentResult};
use cxp_core::customers::CustomersExt;
use cxp_core::feedback::{
    Feedback, FeedbackExt, FeedbackFilter, NewFeedback, NewSurvey, Survey, SurveyResponse,
};
use cxp_core::interactions::parse_json;
use cxp_core::Page;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::{tool, tool_router};
use serde::Deserialize;

use super::out::{self, List};
use super::{optional_uuid, uuid, IdParam};
use crate::server::{Cxp, McpResult};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateSurveyParams {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// A JSON array of question objects, as text. Each has `id` (unique in
    /// the survey), `questionType` (`rating`, `multiple_choice`, or `text`),
    /// `text`, and `required` (true or false); a `multiple_choice` question
    /// also has `options`, an array of strings. Other shapes are stored but
    /// not rendered by the otto-cxp console. There is no scale field: for an
    /// NPS question, use a `rating` asking for 0 to 10 and put it first
    /// (calculate_nps scores a response by its first answer). E.g.
    /// `[{"id": "q1", "questionType": "rating", "text": "How likely are you to
    /// recommend us, from 0 to 10?", "required": true}, {"id": "q2",
    /// "questionType": "multiple_choice", "text": "Which feature do you use
    /// most?", "options": ["Dashboard", "Reports"], "required": true}, {"id":
    /// "q3", "questionType": "text", "text": "What should we improve?",
    /// "required": false}]`.
    pub questions: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendSurveyParams {
    /// The survey's UUID.
    pub survey_id: String,
    /// The customer: a UUID, an exact email, or a name
    /// (matched exactly, case-insensitively, else by a unique partial match of
    /// the name or email; several matches are refused).
    pub customer_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubmitSurveyResponseParams {
    /// The survey's UUID.
    pub survey_id: String,
    /// The customer: a UUID, an exact email, or a name
    /// (matched exactly, case-insensitively, else by a unique partial match of
    /// the name or email; several matches are refused).
    pub customer_id: String,
    /// The answers as JSON text: an array of {questionId, value} objects, or
    /// an object such as {"score": 9}.
    pub answers: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubmitFeedbackParams {
    /// The customer: a UUID, an exact email, or a name
    /// (matched exactly, case-insensitively, else by a unique partial match of
    /// the name or email; several matches are refused).
    pub customer_id: String,
    /// bug_report, feature_request, complaint, praise, or suggestion.
    pub feedback_type: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// 1 to 5.
    #[serde(default)]
    pub rating: Option<i32>,
    /// The catalog experience's UUID.
    #[serde(default)]
    pub experience_id: Option<String>,
    /// The catalog application's UUID.
    #[serde(default)]
    pub application_id: Option<String>,
    /// The catalog page's UUID.
    #[serde(default)]
    pub page_id: Option<String>,
    /// The catalog component's UUID.
    #[serde(default)]
    pub component_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetFeedbackParams {
    /// Only this customer's: a UUID, an exact email, or a name
    /// (matched exactly, case-insensitively, else by a unique partial match of
    /// the name or email; several matches are refused).
    #[serde(default)]
    pub customer_id: Option<String>,
    /// Only feedback of this type: bug_report, feature_request, complaint,
    /// praise, or suggestion.
    #[serde(default)]
    pub feedback_type: Option<String>,
    /// At most this many (1 to 200, default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Skip this many first.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SurveyResponsesParams {
    /// The survey's UUID.
    pub id: String,
    /// At most this many (1 to 200, default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Skip this many first.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CalculateCsatParams {
    /// Only feedback of this type: bug_report, feature_request, complaint,
    /// praise, or suggestion.
    #[serde(default)]
    pub feedback_type: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TextParam {
    pub text: String,
}

#[tool_router(router = feedback_router, vis = "pub(crate)")]
impl Cxp {
    #[tool(
        name = "get_survey",
        description = "Get a survey by ID.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_survey(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<IdParam>,
    ) -> Result<Json<Survey>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let id = uuid("id", &p.id)?;
        let mut tx = self.tx(&caller).await?;
        let survey = tx.get_survey(id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(survey))
    }

    #[tool(
        name = "submit_survey_response",
        description = "Submit a completed survey response with answers for a customer. Accepts \
                       UUID, email, or name for customer_id. Needs cxp:write. Billable.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn submit_survey_response(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<SubmitSurveyResponseParams>,
    ) -> Result<Json<SurveyResponse>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let survey_id = uuid("survey_id", &p.survey_id)?;
        let answers = parse_json("answers", &p.answers).mcp()?;
        let warmed = self.warm_quota(&caller).await;
        let mut tx = self.tx(&caller).await?;
        self.charge(&mut tx, &caller, "submit_survey_response", &warmed)
            .await?;
        let customer_id = tx.resolve_customer(&p.customer_id).await.mcp()?;
        let response = tx
            .submit_survey_response(survey_id, customer_id, &answers)
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(response))
    }

    #[tool(
        name = "create_survey",
        description = "Create a new survey. questions is a JSON array of question objects, as \
                       text: each has id, questionType (rating, multiple_choice, or text), text, \
                       and required; a multiple_choice question also has options. For NPS, put \
                       a 0-to-10 rating question first (calculate_nps scores the first answer). \
                       E.g. [{\"id\": \"q1\", \"questionType\": \"rating\", \"text\": \"How \
                       likely are you to recommend us, from 0 to 10?\", \"required\": true}, \
                       {\"id\": \"q2\", \"questionType\": \"multiple_choice\", \"text\": \"Which \
                       feature do you use most?\", \"options\": [\"Dashboard\", \"Reports\"], \
                       \"required\": true}, {\"id\": \"q3\", \"questionType\": \"text\", \
                       \"text\": \"What should we improve?\", \"required\": false}]. Needs \
                       cxp:write. Billable.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn create_survey(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<CreateSurveyParams>,
    ) -> Result<Json<Survey>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let questions = parse_json("questions", &p.questions).mcp()?;
        let mut tx = self.tx(&caller).await?;
        let survey = tx
            .create_survey(&NewSurvey {
                title: p.title,
                description: p.description,
                questions,
                ..NewSurvey::default()
            })
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(survey))
    }

    #[tool(
        name = "send_survey",
        description = "Send a survey to a customer: an open response awaiting answers. Accepts \
                       UUID, email, or name for customer_id. Needs cxp:write. Billable.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn send_survey(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<SendSurveyParams>,
    ) -> Result<Json<SurveyResponse>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let survey_id = uuid("survey_id", &p.survey_id)?;
        let warmed = self.warm_quota(&caller).await;
        let mut tx = self.tx(&caller).await?;
        self.charge(&mut tx, &caller, "send_survey", &warmed)
            .await?;
        let customer_id = tx.resolve_customer(&p.customer_id).await.mcp()?;
        let response = tx.send_survey(survey_id, customer_id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(response))
    }

    #[tool(
        name = "get_survey_responses",
        description = "Get a survey's responses, newest first. Returns {items, total}.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_survey_responses(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<SurveyResponsesParams>,
    ) -> Result<Json<List<SurveyResponse>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let id = uuid("id", &p.id)?;
        let mut tx = self.tx(&caller).await?;
        let page = tx
            .list_survey_responses(id, Page::new(p.limit, p.offset))
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::counted(page)))
    }

    #[tool(
        name = "submit_feedback",
        description = "Submit customer feedback. Accepts UUID, email, or name for customer_id. \
                       Needs cxp:write. Billable.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn submit_feedback(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<SubmitFeedbackParams>,
    ) -> Result<Json<Feedback>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let experience_id = optional_uuid("experience_id", p.experience_id.as_deref())?;
        let application_id = optional_uuid("application_id", p.application_id.as_deref())?;
        let page_id = optional_uuid("page_id", p.page_id.as_deref())?;
        let component_id = optional_uuid("component_id", p.component_id.as_deref())?;
        let warmed = self.warm_quota(&caller).await;
        let mut tx = self.tx(&caller).await?;
        self.charge(&mut tx, &caller, "submit_feedback", &warmed)
            .await?;
        let customer_id = tx.resolve_customer(&p.customer_id).await.mcp()?;
        let feedback = tx
            .submit_feedback(&NewFeedback {
                customer_id,
                feedback_type: p.feedback_type,
                subject: p.subject,
                body: p.body,
                rating: p.rating,
                experience_id,
                application_id,
                page_id,
                component_id,
            })
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(feedback))
    }

    #[tool(
        name = "get_feedback",
        description = "Get feedback entries, newest first, with optional filters: customer_id \
                       (UUID, email, or name) and feedback_type (bug_report, feature_request, \
                       complaint, praise, suggestion). Returns {items, total}.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_feedback(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<GetFeedbackParams>,
    ) -> Result<Json<List<Feedback>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let customer_id = match p.customer_id.as_deref() {
            Some(c) => Some(tx.resolve_customer(c).await.mcp()?),
            None => None,
        };
        let page = tx
            .list_feedback(
                &FeedbackFilter {
                    customer_id,
                    feedback_type: p.feedback_type,
                    ..FeedbackFilter::default()
                },
                Page::new(p.limit, p.offset),
            )
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::counted(page)))
    }

    #[tool(
        name = "calculate_nps",
        description = "Calculate the Net Promoter Score for a survey. A response's score is the \
                       first answer's `value` when its answers are an array (as \
                       submit_survey_response's [{questionId, value}] answers are, so a survey \
                       made for NPS puts its 0-to-10 rating question first), or the `score` key \
                       when they are an object; scores outside 0 to 10, or missing, are not \
                       counted.",
        annotations(read_only_hint = true)
    )]
    pub async fn calculate_nps(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<IdParam>,
    ) -> Result<Json<out::NpsOut>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let id = uuid("id", &p.id)?;
        let mut tx = self.tx(&caller).await?;
        let counts = tx.survey_nps_counts(id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(out::NpsOut {
            nps: counts.nps(),
            counts,
        }))
    }

    #[tool(
        name = "calculate_csat",
        description = "Calculate the Customer Satisfaction Score (percent of ratings 4 or 5) \
                       over rated feedback, optionally of one feedback_type (bug_report, \
                       feature_request, complaint, praise, suggestion).",
        annotations(read_only_hint = true)
    )]
    pub async fn calculate_csat(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<CalculateCsatParams>,
    ) -> Result<Json<out::CsatOut>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let counts = tx
            .feedback_csat_counts(p.feedback_type.as_deref())
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(out::CsatOut {
            csat: counts.csat(),
            counts,
        }))
    }

    #[tool(
        name = "analyze_sentiment",
        description = "Analyze the sentiment of a text string (keyword based): a score from -1 to \
                       1, a label, and the words that drove it.",
        annotations(read_only_hint = true)
    )]
    pub async fn analyze_sentiment(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<TextParam>,
    ) -> Result<Json<SentimentResult>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        Ok(Json(analyze_sentiment(&p.text)))
    }
}
