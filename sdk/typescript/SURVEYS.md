# Surveys

Surveys in CXP let you define a set of questions, send them to customers, collect responses, and calculate metrics like NPS.

## Data Model

### Survey

A survey is a named collection of questions stored as JSONB.

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID | Auto-generated |
| `title` | string | Display title |
| `description` | string? | Optional explanation shown to the customer |
| `questions` | SurveyQuestion[] | Ordered list of questions |
| `active` | boolean | Whether the survey accepts new responses |
| `createdAt` | timestamp | |
| `updatedAt` | timestamp | |

### SurveyQuestion

Each question lives inside the survey's `questions` JSON array.

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique within the survey (e.g. `"q1"`) |
| `text` | string | The question prompt |
| `questionType` | `"rating"` \| `"text"` \| `"multiple_choice"` | Controls how the question is rendered and answered |
| `options` | string[]? | Required for `multiple_choice`; ignored for other types |
| `required` | boolean | Whether the customer must answer before submitting |

### SurveyResponse

Created when a survey is sent to a customer, then updated when they submit answers.

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID | Auto-generated |
| `surveyId` | UUID | References the parent survey |
| `customerId` | UUID | The customer who received the survey |
| `answers` | SurveyAnswer[] | Empty until the customer submits |
| `completedAt` | timestamp? | Set when answers are submitted |
| `createdAt` | timestamp | When the survey was sent |

### SurveyAnswer

Each answer pairs a question ID with a string value.

```ts
{ questionId: "q1", value: "9" }      // rating
{ questionId: "q2", value: "Great!" } // text
{ questionId: "q3", value: "Email" }  // multiple_choice
```

## Lifecycle

```
create survey  ──>  send to customer  ──>  customer submits answers
                                                    │
                                              calculate NPS
```

1. **Create** a survey with a title and questions.
2. **Send** it to one or more customers. Each send creates a `SurveyResponse` with empty answers.
3. **Submit** answers. The response is updated and `completedAt` is set.
4. **Analyze** — call `calculateNps` on surveys that use rating questions (1-10 scale).

## SDK Usage

All survey operations are on `cxp.feedback`.

### Create a survey

```ts
const survey = await cxp.feedback.createSurvey(
  "Post-Purchase NPS",
  [
    {
      id: "q1",
      text: "How likely are you to recommend us to a friend? (1-10)",
      questionType: "rating",
      required: true,
    },
    {
      id: "q2",
      text: "What could we do better?",
      questionType: "text",
      required: false,
    },
    {
      id: "q3",
      text: "How did you hear about us?",
      questionType: "multiple_choice",
      options: ["Search engine", "Social media", "Friend", "Email", "Other"],
      required: true,
    },
  ],
  "We'd love your feedback on your recent purchase." // optional description
);

console.log(survey.id); // UUID of the new survey
```

### Send a survey to a customer

```ts
const response = await cxp.feedback.sendSurvey(survey.id, customerId);
// response.id is the SurveyResponse UUID — answers are empty at this point
```

### Submit answers

```ts
await cxp.feedback.submitSurveyResponse({
  surveyId: survey.id,
  customerId,
  answers: [
    { questionId: "q1", value: "9" },
    { questionId: "q2", value: "Faster shipping would be great" },
    { questionId: "q3", value: "Friend" },
  ],
});
```

### Retrieve responses

```ts
const responses = await cxp.feedback.getResponses(survey.id);

for (const r of responses) {
  console.log(r.customerId, r.completedAt ? "completed" : "pending");
}
```

### Fetch a survey by ID

```ts
const survey = await cxp.feedback.getSurvey(surveyId);
console.log(survey.title, survey.questions.length, "questions");
```

### Calculate NPS

NPS works on rating questions with a 1-10 scale. Responses are bucketed as:

- **Promoters** (9-10)
- **Passives** (7-8)
- **Detractors** (1-6)

NPS = % Promoters - % Detractors (range: -100 to 100).

```ts
const nps = await cxp.feedback.calculateNps(survey.id);
console.log("NPS:", nps.score); // e.g. 42
```

## React Component

The `@cxp/react` package includes a `<SurveyRenderer>` component that handles fetching, rendering, and submission.

```tsx
import { SurveyRenderer } from "@cxp/react";

<SurveyRenderer
  surveyId="survey-uuid"
  customerId="customer-uuid"
  onComplete={() => console.log("Done!")}
/>
```

It renders each question type with appropriate inputs:

- **rating** — row of 1-10 buttons
- **text** — textarea
- **multiple_choice** — radio button list

## Question Type Reference

| Type | Input | `value` format | Example |
|------|-------|---------------|---------|
| `rating` | 1-10 button row | Numeric string | `"9"` |
| `text` | Textarea | Free-form string | `"Great experience"` |
| `multiple_choice` | Radio buttons | One of the `options` strings | `"Email"` |

## MCP Tools

These are the underlying MCP tools that the SDK wraps:

| Tool | Description |
|------|-------------|
| `create_survey` | Create a survey with a title, optional description, and JSON questions |
| `get_survey` | Fetch a survey by UUID |
| `send_survey` | Send a survey to a customer (creates an empty response record) |
| `submit_survey_response` | Submit answers for a customer's survey response |
| `get_survey_responses` | List all responses for a survey |
| `calculate_nps` | Calculate NPS from a survey's completed rating responses |
