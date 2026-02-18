import { useSearchParams } from "react-router";
import { useFeedback } from "@/hooks/use-feedback";
import { PageHeader } from "@/components/shared/page-header";
import { LoadingTable } from "@/components/shared/loading-state";
import { EmptyState } from "@/components/shared/empty-state";
import { Badge } from "@/components/ui/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { MessageSquare, Star } from "lucide-react";
import { useState, useCallback } from "react";
import {
  CatalogFilterBar,
  type CatalogFilterValues,
} from "@/components/shared/catalog-filter-bar";

const PAGE_SIZE = 20;

const FEEDBACK_TYPES = [
  { value: "bug_report", label: "Bug Report" },
  { value: "feature_request", label: "Feature Request" },
  { value: "complaint", label: "Complaint" },
  { value: "praise", label: "Praise" },
  { value: "suggestion", label: "Suggestion" },
];

function feedbackTypeBadgeVariant(type: string) {
  switch (type) {
    case "complaint":
      return "destructive";
    case "praise":
      return "default";
    case "bug_report":
      return "outline";
    default:
      return "secondary";
  }
}

function feedbackTypeLabel(type: string) {
  return FEEDBACK_TYPES.find((t) => t.value === type)?.label ?? type;
}

function RatingStars({ rating }: { rating: number | null }) {
  if (rating === null) return <span className="text-muted-foreground text-xs">—</span>;
  return (
    <span className="flex items-center gap-0.5">
      {Array.from({ length: 5 }).map((_, i) => (
        <Star
          key={i}
          className={`h-3 w-3 ${i < rating ? "fill-amber-400 text-amber-400" : "text-muted-foreground/30"}`}
        />
      ))}
      <span className="ml-1 text-xs text-muted-foreground">{rating}/5</span>
    </span>
  );
}

export function FeedbackPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [page, setPage] = useState(0);

  const feedbackType = searchParams.get("feedback_type") ?? undefined;

  const catalogFilters: CatalogFilterValues = {
    experience_id: searchParams.get("experience_id") ?? undefined,
    application_id: searchParams.get("application_id") ?? undefined,
    page_id: searchParams.get("page_id") ?? undefined,
    component_id: searchParams.get("component_id") ?? undefined,
  };

  const handleCatalogChange = useCallback(
    (values: CatalogFilterValues) => {
      setSearchParams((prev) => {
        const next = new URLSearchParams(prev);
        const keys = [
          "experience_id",
          "application_id",
          "page_id",
          "component_id",
        ] as const;
        for (const key of keys) {
          if (values[key]) {
            next.set(key, values[key]);
          } else {
            next.delete(key);
          }
        }
        return next;
      });
      setPage(0);
    },
    [setSearchParams],
  );

  const { data, isLoading } = useFeedback({
    feedback_type: feedbackType,
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
    ...catalogFilters,
  });

  const items = data?.data ?? [];
  const total = data?.total ?? 0;
  const totalPages = Math.ceil(total / PAGE_SIZE);

  function setFilter(key: string, value: string | undefined) {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (value) {
        next.set(key, value);
      } else {
        next.delete(key);
      }
      return next;
    });
    setPage(0);
  }

  return (
    <div className="space-y-6">
      <PageHeader title="Feedback" />

      {/* Filter row */}
      <div className="flex flex-wrap items-center gap-3">
        <Select
          value={feedbackType ?? "all"}
          onValueChange={(v) => setFilter("feedback_type", v === "all" ? undefined : v)}
        >
          <SelectTrigger className="w-[180px]" aria-label="Filter by type">
            <SelectValue placeholder="All types" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All types</SelectItem>
            {FEEDBACK_TYPES.map((t) => (
              <SelectItem key={t.value} value={t.value}>
                {t.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {feedbackType && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setFilter("feedback_type", undefined)}
          >
            Clear filter
          </Button>
        )}
      </div>

      {/* Catalog filters */}
      <CatalogFilterBar
        value={catalogFilters}
        onChange={handleCatalogChange}
      />

      {isLoading ? (
        <LoadingTable />
      ) : items.length === 0 ? (
        <EmptyState
          icon={MessageSquare}
          title="No feedback found"
          description="Adjust your filters or check back later."
        />
      ) : (
        <>
          <div className="rounded-md border overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Type</TableHead>
                  <TableHead>Subject</TableHead>
                  <TableHead>Rating</TableHead>
                  <TableHead>Customer ID</TableHead>
                  <TableHead className="hidden md:table-cell">Created At</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {items.map((item) => (
                  <TableRow key={item.id}>
                    <TableCell>
                      <Badge variant={feedbackTypeBadgeVariant(item.feedback_type)}>
                        {feedbackTypeLabel(item.feedback_type)}
                      </Badge>
                    </TableCell>
                    <TableCell className="max-w-[200px] truncate">
                      {item.subject ?? <span className="text-muted-foreground text-xs">—</span>}
                    </TableCell>
                    <TableCell>
                      <RatingStars rating={item.rating} />
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      {item.customer_id.slice(0, 8)}
                    </TableCell>
                    <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                      {new Date(item.created_at).toLocaleDateString()}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>

          {/* Pagination */}
          {totalPages > 1 && (
            <div className="flex items-center justify-between text-sm text-muted-foreground">
              <span>
                Showing {page * PAGE_SIZE + 1}–{Math.min((page + 1) * PAGE_SIZE, total)} of {total}
              </span>
              <div className="flex gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setPage((p) => p - 1)}
                  disabled={page === 0}
                >
                  Previous
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setPage((p) => p + 1)}
                  disabled={page >= totalPages - 1}
                >
                  Next
                </Button>
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
