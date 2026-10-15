/**
 * TicketDetailPage - Detail view for a single support ticket.
 *
 * Usage (App.tsx):
 *   <Route path="tickets/:id" element={<TicketDetailPage />} />
 */

import { useParams, useNavigate, Link } from "react-router";
import { toast } from "sonner";
import { ArrowLeft, ExternalLink } from "lucide-react";

import { useTicket, useUpdateTicket } from "@/hooks/use-tickets";
import { StatusBadge } from "@/components/shared/status-badge";
import { PriorityBadge } from "@/components/shared/priority-badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const STATUS_OPTIONS = [
  { value: "open", label: "Open" },
  { value: "in_progress", label: "In Progress" },
  { value: "waiting", label: "Waiting" },
  { value: "resolved", label: "Resolved" },
  { value: "closed", label: "Closed" },
];

const PRIORITY_OPTIONS = [
  { value: "urgent", label: "Urgent" },
  { value: "high", label: "High" },
  { value: "medium", label: "Medium" },
  { value: "low", label: "Low" },
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatDateTime(iso: string) {
  return new Date(iso).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// ---------------------------------------------------------------------------
// Skeleton
// ---------------------------------------------------------------------------

function TicketDetailSkeleton() {
  return (
    <div className="space-y-6" aria-busy="true" aria-label="Loading ticket">
      {/* Back button placeholder */}
      <Skeleton className="h-9 w-28" />

      <div className="grid gap-6 lg:grid-cols-3">
        {/* Left column */}
        <div className="lg:col-span-2 space-y-6">
          <Skeleton className="h-8 w-3/4" />
          <Card>
            <CardHeader>
              <Skeleton className="h-5 w-24" />
            </CardHeader>
            <CardContent className="space-y-3">
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-2/3" />
            </CardContent>
          </Card>
        </div>

        {/* Right column */}
        <div>
          <Card>
            <CardHeader>
              <Skeleton className="h-5 w-20" />
            </CardHeader>
            <CardContent className="space-y-4">
              {Array.from({ length: 5 }).map((_, i) => (
                <div key={i} className="space-y-1.5">
                  <Skeleton className="h-4 w-20" />
                  <Skeleton className="h-10 w-full" />
                </div>
              ))}
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// TicketDetailPage
// ---------------------------------------------------------------------------

export function TicketDetailPage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();

  const { data: ticket, isLoading, isError } = useTicket(id);
  const updateTicket = useUpdateTicket();

  // ---------------------------------------------------------------------------
  // Update helper — saves a single field immediately
  // ---------------------------------------------------------------------------

  async function updateField(
    field: "status" | "priority" | "assignedTo",
    value: string
  ) {
    if (!ticket) return;
    try {
      await updateTicket.mutateAsync({
        id: ticket.id,
        data: { [field]: value || undefined },
      });
      toast.success("Ticket updated");
    } catch {
      toast.error("Failed to update ticket");
    }
  }

  // ---------------------------------------------------------------------------
  // Loading / error states
  // ---------------------------------------------------------------------------

  if (isLoading) return <TicketDetailSkeleton />;

  if (isError || !ticket) {
    return (
      <div className="space-y-4">
        <Button variant="ghost" onClick={() => navigate("/tickets")}>
          <ArrowLeft className="h-4 w-4 mr-2" />
          Back to Tickets
        </Button>
        <p className="text-destructive">
          Ticket not found or failed to load.
        </p>
      </div>
    );
  }

  // ---------------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------------

  return (
    <div className="space-y-6">
      {/* Back button */}
      <Button
        variant="ghost"
        size="sm"
        onClick={() => navigate("/tickets")}
        className="gap-2"
        aria-label="Back to tickets list"
      >
        <ArrowLeft className="h-4 w-4" />
        Back to Tickets
      </Button>

      {/* Two-column layout */}
      <div className="grid gap-6 lg:grid-cols-3">
        {/* ---------------------------------------------------------------- */}
        {/* Left column — subject + description                              */}
        {/* ---------------------------------------------------------------- */}
        <div className="lg:col-span-2 space-y-6">
          <div className="space-y-2">
            <div className="flex flex-wrap items-center gap-3">
              <StatusBadge status={ticket.status} />
              <PriorityBadge priority={ticket.priority} />
            </div>
            <h1 className="text-2xl font-bold tracking-tight leading-tight">
              {ticket.subject}
            </h1>
          </div>

          <Card>
            <CardHeader>
              <CardTitle className="text-base font-semibold">
                Description
              </CardTitle>
            </CardHeader>
            <CardContent>
              {ticket.description ? (
                <p className="text-sm leading-relaxed whitespace-pre-wrap text-foreground">
                  {ticket.description}
                </p>
              ) : (
                <p className="text-sm text-muted-foreground italic">
                  No description provided.
                </p>
              )}
            </CardContent>
          </Card>

          {/* Tags */}
          {ticket.tags && ticket.tags.length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle className="text-base font-semibold">Tags</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="flex flex-wrap gap-2">
                  {ticket.tags.map((tag) => (
                    <Badge key={tag} variant="secondary">
                      {tag}
                    </Badge>
                  ))}
                </div>
              </CardContent>
            </Card>
          )}
        </div>

        {/* ---------------------------------------------------------------- */}
        {/* Right column — properties panel                                  */}
        {/* ---------------------------------------------------------------- */}
        <div>
          <Card>
            <CardHeader>
              <CardTitle className="text-base font-semibold">
                Properties
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-5">
              {/* Status */}
              <div className="space-y-1.5">
                <Label htmlFor="detail-status">Status</Label>
                <Select
                  value={ticket.status}
                  onValueChange={(val) => updateField("status", val)}
                  disabled={updateTicket.isPending}
                >
                  <SelectTrigger
                    id="detail-status"
                    aria-label="Change ticket status"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {STATUS_OPTIONS.map((opt) => (
                      <SelectItem key={opt.value} value={opt.value}>
                        {opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              {/* Priority */}
              <div className="space-y-1.5">
                <Label htmlFor="detail-priority">Priority</Label>
                <Select
                  value={ticket.priority}
                  onValueChange={(val) => updateField("priority", val)}
                  disabled={updateTicket.isPending}
                >
                  <SelectTrigger
                    id="detail-priority"
                    aria-label="Change ticket priority"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {PRIORITY_OPTIONS.map((opt) => (
                      <SelectItem key={opt.value} value={opt.value}>
                        {opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              {/* Assigned To */}
              <div className="space-y-1.5">
                <Label htmlFor="detail-assigned">Assigned To</Label>
                <Input
                  id="detail-assigned"
                  defaultValue={ticket.assignedTo ?? ""}
                  placeholder="Unassigned"
                  onBlur={(e) => {
                    const val = e.currentTarget.value.trim();
                    if (val !== (ticket.assignedTo ?? "")) {
                      updateField("assignedTo", val);
                    }
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.currentTarget.blur();
                    }
                  }}
                  disabled={updateTicket.isPending}
                  aria-label="Assigned to"
                />
              </div>

              <Separator />

              {/* Channel — display only */}
              <div className="space-y-1.5">
                <p className="text-sm font-medium">Channel</p>
                {ticket.channel ? (
                  <Badge variant="outline" className="capitalize">
                    {ticket.channel}
                  </Badge>
                ) : (
                  <p className="text-sm text-muted-foreground">-</p>
                )}
              </div>

              {/* Customer ID */}
              <div className="space-y-1.5">
                <p className="text-sm font-medium">Customer</p>
                <Link
                  to={`/customers/${ticket.customerId}`}
                  className="inline-flex items-center gap-1 text-sm font-mono text-primary hover:underline focus:outline-none focus:ring-2 focus:ring-ring rounded"
                  aria-label={`View customer ${ticket.customerId}`}
                >
                  {ticket.customerId.slice(0, 8)}...
                  <ExternalLink className="h-3 w-3 shrink-0" />
                </Link>
              </div>

              <Separator />

              {/* Timestamps */}
              <div className="space-y-3">
                <div>
                  <p className="text-xs font-medium text-muted-foreground uppercase tracking-wide">
                    Created
                  </p>
                  <p className="text-sm mt-0.5">
                    {formatDateTime(ticket.createdAt)}
                  </p>
                </div>
                <div>
                  <p className="text-xs font-medium text-muted-foreground uppercase tracking-wide">
                    Last Updated
                  </p>
                  <p className="text-sm mt-0.5">
                    {formatDateTime(ticket.updatedAt)}
                  </p>
                </div>
                {ticket.resolvedAt && (
                  <div>
                    <p className="text-xs font-medium text-muted-foreground uppercase tracking-wide">
                      Resolved
                    </p>
                    <p className="text-sm mt-0.5">
                      {formatDateTime(ticket.resolvedAt)}
                    </p>
                  </div>
                )}
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
