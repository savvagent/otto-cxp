import { useState, useCallback } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { ClipboardList, Pencil, Trash2 } from "lucide-react";

import {
  useSurveys,
  useCreateSurvey,
  useDeleteSurvey,
} from "@/hooks/use-surveys";
import { PageHeader } from "@/components/shared/page-header";
import { LoadingTable } from "@/components/shared/loading-state";
import { EmptyState } from "@/components/shared/empty-state";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import type { Survey } from "@/lib/types";
import {
  CatalogFilterBar,
  type CatalogFilterValues,
  catalogFiltersFromUrl,
  withCatalogFilters,
} from "@/components/shared/catalog-filter-bar";

const PAGE_SIZE = 20;

const createSurveySchema = z.object({
  title: z.string().min(1, "Title is required"),
  description: z.string().optional(),
  questionsRaw: z.string().optional(),
});

type CreateSurveyForm = z.infer<typeof createSurveySchema>;

function CreateSurveyDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const createSurvey = useCreateSurvey();
  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<CreateSurveyForm>({
    resolver: zodResolver(createSurveySchema),
  });

  function onSubmit(values: CreateSurveyForm) {
    let questions: unknown = [];
    if (values.questionsRaw?.trim()) {
      try {
        questions = JSON.parse(values.questionsRaw);
      } catch {
        toast.error("Questions must be valid JSON");
        return;
      }
    }

    createSurvey.mutate(
      {
        title: values.title,
        description: values.description || undefined,
        questions,
      },
      {
        onSuccess: () => {
          toast.success("Survey created");
          reset();
          onOpenChange(false);
        },
        onError: (err) => {
          toast.error(`Failed to create survey: ${err.message}`);
        },
      }
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New Survey</DialogTitle>
        </DialogHeader>
        <form onSubmit={handleSubmit(onSubmit)} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="survey-title">
              Title <span className="text-destructive">*</span>
            </Label>
            <Input
              id="survey-title"
              placeholder="Customer Satisfaction Survey"
              {...register("title")}
            />
            {errors.title && (
              <p className="text-xs text-destructive">{errors.title.message}</p>
            )}
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="survey-description">Description</Label>
            <Textarea
              id="survey-description"
              placeholder="Optional description..."
              rows={3}
              {...register("description")}
            />
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="survey-questions">Questions (JSON)</Label>
            <Textarea
              id="survey-questions"
              placeholder='[{"id": "q1", "text": "How satisfied are you?", "type": "rating"}]'
              rows={4}
              className="font-mono text-xs"
              {...register("questionsRaw")}
            />
            <p className="text-xs text-muted-foreground">
              Optional. Paste a JSON array of question objects.
            </p>
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={createSurvey.isPending}>
              {createSurvey.isPending ? "Creating..." : "Create Survey"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function SurveysPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [page] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Survey | null>(null);

  const catalogFilters = catalogFiltersFromUrl(searchParams);

  const handleCatalogChange = useCallback(
    (values: CatalogFilterValues) => {
      setSearchParams((prev) => withCatalogFilters(prev, values));
    },
    [setSearchParams],
  );

  const { data, isLoading } = useSurveys({
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
    ...catalogFilters,
  });

  const deleteSurvey = useDeleteSurvey();

  const items = data?.data ?? [];
  const total = data?.total ?? 0;

  function handleDelete() {
    if (!deleteTarget) return;
    deleteSurvey.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast.success("Survey deleted");
        setDeleteTarget(null);
      },
      onError: (err) => {
        toast.error(`Failed to delete: ${err.message}`);
      },
    });
  }

  function questionCount(questions: unknown): number {
    if (Array.isArray(questions)) return questions.length;
    return 0;
  }

  return (
    <div className="space-y-6">
      <PageHeader
        title="Surveys"
        action="New Survey"
        onAction={() => setCreateOpen(true)}
      />

      {/* Catalog filters */}
      <CatalogFilterBar
        value={catalogFilters}
        onChange={handleCatalogChange}
      />

      {isLoading ? (
        <LoadingTable />
      ) : items.length === 0 ? (
        <EmptyState
          icon={ClipboardList}
          title="No surveys yet"
          description="Create your first survey to start collecting responses."
          action="New Survey"
          onAction={() => setCreateOpen(true)}
        />
      ) : (
        <div className="rounded-md border overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Title</TableHead>
                <TableHead className="hidden sm:table-cell">Description</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Questions</TableHead>
                <TableHead className="hidden md:table-cell">Created At</TableHead>
                <TableHead className="w-[80px]" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((survey) => (
                <TableRow
                  key={survey.id}
                  className="cursor-pointer"
                  onClick={() => navigate(`/surveys/${survey.id}`)}
                >
                  <TableCell className="font-medium">{survey.title}</TableCell>
                  <TableCell className="hidden sm:table-cell max-w-[240px] truncate text-muted-foreground text-sm">
                    {survey.description ?? "—"}
                  </TableCell>
                  <TableCell>
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
                  </TableCell>
                  <TableCell>{questionCount(survey.questions)}</TableCell>
                  <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                    {new Date(survey.createdAt).toLocaleDateString()}
                  </TableCell>
                  <TableCell>
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label="Delete survey"
                      onClick={(e) => {
                        e.stopPropagation();
                        setDeleteTarget(survey);
                      }}
                    >
                      <Trash2 className="h-4 w-4 text-muted-foreground" />
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}

      {total > PAGE_SIZE && (
        <p className="text-sm text-muted-foreground">
          Showing {Math.min(PAGE_SIZE, items.length)} of {total} surveys
        </p>
      )}

      <CreateSurveyDialog open={createOpen} onOpenChange={setCreateOpen} />

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title="Delete Survey"
        description={`Are you sure you want to delete "${deleteTarget?.title}"? This action cannot be undone.`}
        onConfirm={handleDelete}
        loading={deleteSurvey.isPending}
      />
    </div>
  );
}
