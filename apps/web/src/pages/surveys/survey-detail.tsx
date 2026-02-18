import { useParams, Link } from "react-router";
import { useSurvey, useSurveyResponses } from "@/hooks/use-surveys";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { LoadingTable } from "@/components/shared/loading-state";
import { EmptyState } from "@/components/shared/empty-state";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ArrowLeft, ClipboardList, ListOrdered } from "lucide-react";

function QuestionsList({ questions }: { questions: unknown }) {
  let items: Array<Record<string, unknown>> = [];
  if (Array.isArray(questions)) {
    items = questions as Array<Record<string, unknown>>;
  }

  if (items.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">No questions defined.</p>
    );
  }

  return (
    <ol className="space-y-2">
      {items.map((q, i) => {
        const text =
          (q.text as string) ??
          (q.question as string) ??
          (q.label as string) ??
          JSON.stringify(q);
        const type = q.type as string | undefined;
        return (
          <li key={i} className="flex items-start gap-3 text-sm">
            <span className="flex-shrink-0 flex h-6 w-6 items-center justify-center rounded-full bg-muted text-xs font-medium">
              {i + 1}
            </span>
            <span className="flex-1">
              {text}
              {type && (
                <Badge variant="outline" className="ml-2 text-xs">
                  {type}
                </Badge>
              )}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

export function SurveyDetailPage() {
  const { id } = useParams<{ id: string }>();
  const { data: survey, isLoading: surveyLoading } = useSurvey(id);
  const { data: responsesData, isLoading: responsesLoading } =
    useSurveyResponses(id);

  const responses = responsesData ?? [];

  return (
    <div className="space-y-6">
      {/* Back link */}
      <Link
        to="/surveys"
        className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
      >
        <ArrowLeft className="h-4 w-4" />
        Back to Surveys
      </Link>

      {/* Survey info */}
      <Card>
        <CardHeader>
          {surveyLoading ? (
            <div className="space-y-2">
              <Skeleton className="h-7 w-64" />
              <Skeleton className="h-4 w-40" />
            </div>
          ) : survey ? (
            <>
              <div className="flex items-center justify-between gap-4 flex-wrap">
                <CardTitle className="text-xl">{survey.title}</CardTitle>
                <Badge
                  variant={survey.active ? "default" : "secondary"}
                  className={
                    survey.active
                      ? "bg-green-100 text-green-800 border-green-200 hover:bg-green-100"
                      : ""
                  }
                >
                  {survey.active ? "Active" : "Inactive"}
                </Badge>
              </div>
              {survey.description && (
                <CardDescription>{survey.description}</CardDescription>
              )}
              <p className="text-xs text-muted-foreground">
                Created {new Date(survey.created_at).toLocaleDateString()}
              </p>
            </>
          ) : (
            <CardTitle>Survey not found</CardTitle>
          )}
        </CardHeader>
      </Card>

      {/* Questions */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <ListOrdered className="h-4 w-4 text-muted-foreground" />
            <CardTitle className="text-base">Questions</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          {surveyLoading ? (
            <div className="space-y-2">
              {[1, 2, 3].map((i) => (
                <Skeleton key={i} className="h-6 w-full" />
              ))}
            </div>
          ) : (
            <QuestionsList questions={survey?.questions} />
          )}
        </CardContent>
      </Card>

      {/* Responses */}
      <div className="space-y-3">
        <h3 className="text-lg font-semibold">Responses</h3>
        {responsesLoading ? (
          <LoadingTable />
        ) : responses.length === 0 ? (
          <EmptyState
            icon={ClipboardList}
            title="No responses yet"
            description="Responses will appear here once customers complete the survey."
          />
        ) : (
          <div className="rounded-md border overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Customer ID</TableHead>
                  <TableHead>Completed At</TableHead>
                  <TableHead>Answers</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {responses.map((response: import("@/lib/types").SurveyResponse) => {
                  const answersStr = JSON.stringify(response.answers);
                  const truncated =
                    answersStr.length > 80
                      ? answersStr.slice(0, 80) + "…"
                      : answersStr;
                  return (
                    <TableRow key={response.id}>
                      <TableCell className="font-mono text-xs">
                        {response.customer_id.slice(0, 8)}
                      </TableCell>
                      <TableCell className="text-sm text-muted-foreground">
                        {response.completed_at
                          ? new Date(response.completed_at).toLocaleString()
                          : "—"}
                      </TableCell>
                      <TableCell
                        className="font-mono text-xs max-w-[300px] truncate"
                        title={answersStr}
                      >
                        {truncated}
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          </div>
        )}
      </div>
    </div>
  );
}
