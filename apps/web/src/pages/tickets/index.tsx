/**
 * TicketsPage - Full CRUD list view for support tickets.
 *
 * Usage (App.tsx):
 *   <Route path="tickets" element={<TicketsPage />} />
 */

import { useState, useCallback } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Ticket as TicketIcon, FilterX, ChevronLeft, ChevronRight, Trash2 } from "lucide-react";

import {
  useTickets,
  useCreateTicket,
  useDeleteTicket,
} from "@/hooks/use-tickets";
import type { Ticket } from "@/lib/types";
import { PageHeader } from "@/components/shared/page-header";
import { StatusBadge } from "@/components/shared/status-badge";
import { PriorityBadge } from "@/components/shared/priority-badge";
import { EmptyState } from "@/components/shared/empty-state";
import { LoadingTable } from "@/components/shared/loading-state";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import {
  CatalogFilterBar,
  type CatalogFilterValues,
} from "@/components/shared/catalog-filter-bar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

// ---------------------------------------------------------------------------
// Form schema
// ---------------------------------------------------------------------------

const createTicketSchema = z.object({
  customer_id: z.string().min(1, "Customer ID is required"),
  subject: z.string().min(1, "Subject is required"),
  description: z.string().optional(),
  priority: z.string().optional(),
  channel: z.string().optional(),
});

type CreateTicketForm = z.infer<typeof createTicketSchema>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PAGE_SIZE = 20;

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

const CHANNEL_OPTIONS = [
  { value: "email", label: "Email" },
  { value: "phone", label: "Phone" },
  { value: "chat", label: "Chat" },
  { value: "web", label: "Web" },
];

// ---------------------------------------------------------------------------
// TicketsPage
// ---------------------------------------------------------------------------

export function TicketsPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();

  // Derived filter/pagination state from URL
  const statusFilter = searchParams.get("status") ?? "";
  const priorityFilter = searchParams.get("priority") ?? "";
  const page = parseInt(searchParams.get("page") ?? "0", 10);
  const offset = page * PAGE_SIZE;

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
        next.delete("page");
        return next;
      });
    },
    [setSearchParams],
  );

  // Dialog state
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Ticket | null>(null);

  // Data hooks
  const { data, isLoading, isError } = useTickets({
    limit: PAGE_SIZE,
    offset,
    status: statusFilter || undefined,
    priority: priorityFilter || undefined,
    ...catalogFilters,
  });

  const createTicket = useCreateTicket();
  const deleteTicket = useDeleteTicket();

  // Form
  const form = useForm<CreateTicketForm>({
    resolver: zodResolver(createTicketSchema),
    defaultValues: {
      customer_id: "",
      subject: "",
      description: "",
      priority: "",
      channel: "",
    },
  });

  // ---------------------------------------------------------------------------
  // Handlers
  // ---------------------------------------------------------------------------

  function setFilter(key: "status" | "priority", value: string) {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (value) {
        next.set(key, value);
      } else {
        next.delete(key);
      }
      next.delete("page"); // reset pagination on filter change
      return next;
    });
  }

  function clearFilters() {
    setSearchParams({});
  }

  function setPage(next: number) {
    setSearchParams((prev) => {
      const params = new URLSearchParams(prev);
      if (next === 0) {
        params.delete("page");
      } else {
        params.set("page", String(next));
      }
      return params;
    });
  }

  function handleRowClick(ticket: Ticket) {
    navigate(`/tickets/${ticket.id}`);
  }

  async function handleCreate(values: CreateTicketForm) {
    try {
      await createTicket.mutateAsync({
        customer_id: values.customer_id,
        subject: values.subject,
        description: values.description || undefined,
        priority: values.priority || undefined,
        channel: values.channel || undefined,
      });
      toast.success("Ticket created");
      setCreateOpen(false);
      form.reset();
    } catch {
      toast.error("Failed to create ticket");
    }
  }

  async function handleDelete() {
    if (!deleteTarget) return;
    try {
      await deleteTicket.mutateAsync(deleteTarget.id);
      toast.success("Ticket deleted");
      setDeleteTarget(null);
    } catch {
      toast.error("Failed to delete ticket");
    }
  }

  // ---------------------------------------------------------------------------
  // Derived values
  // ---------------------------------------------------------------------------

  const tickets = data?.data ?? [];
  const total = data?.total ?? 0;
  const hasFilters = !!(
    statusFilter ||
    priorityFilter ||
    catalogFilters.experience_id ||
    catalogFilters.application_id ||
    catalogFilters.page_id ||
    catalogFilters.component_id
  );
  const totalPages = Math.ceil(total / PAGE_SIZE);
  const hasPrev = page > 0;
  const hasNext = page < totalPages - 1;

  // ---------------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------------

  return (
    <div className="space-y-6">
      {/* Header */}
      <PageHeader
        title="Tickets"
        description="Manage customer support tickets"
        action="New Ticket"
        onAction={() => setCreateOpen(true)}
      />

      {/* Filter bar */}
      <div className="flex flex-wrap items-center gap-3">
        <Select
          value={statusFilter}
          onValueChange={(val) => setFilter("status", val === "_all" ? "" : val)}
        >
          <SelectTrigger className="w-40" aria-label="Filter by status">
            <SelectValue placeholder="All statuses" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="_all">All statuses</SelectItem>
            {STATUS_OPTIONS.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <Select
          value={priorityFilter}
          onValueChange={(val) =>
            setFilter("priority", val === "_all" ? "" : val)
          }
        >
          <SelectTrigger className="w-40" aria-label="Filter by priority">
            <SelectValue placeholder="All priorities" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="_all">All priorities</SelectItem>
            {PRIORITY_OPTIONS.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        {hasFilters && (
          <Button
            variant="ghost"
            size="sm"
            onClick={clearFilters}
            className="gap-1.5"
          >
            <FilterX className="h-4 w-4" />
            Clear filters
          </Button>
        )}

        {!isLoading && (
          <span className="ml-auto text-sm text-muted-foreground">
            {total} ticket{total !== 1 ? "s" : ""}
          </span>
        )}
      </div>

      {/* Catalog filters */}
      <CatalogFilterBar
        value={catalogFilters}
        onChange={handleCatalogChange}
      />

      {/* Table */}
      {isLoading ? (
        <LoadingTable rows={8} />
      ) : isError ? (
        <EmptyState
          icon={TicketIcon}
          title="Failed to load tickets"
          description="There was an error fetching tickets. Please try again."
        />
      ) : tickets.length === 0 ? (
        <EmptyState
          icon={TicketIcon}
          title="No tickets found"
          description={
            hasFilters
              ? "No tickets match the current filters."
              : "Create your first support ticket to get started."
          }
          action={hasFilters ? "Clear filters" : "New Ticket"}
          onAction={hasFilters ? clearFilters : () => setCreateOpen(true)}
        />
      ) : (
        <div className="rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Subject</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Priority</TableHead>
                <TableHead className="hidden sm:table-cell">Customer ID</TableHead>
                <TableHead className="hidden md:table-cell">Channel</TableHead>
                <TableHead className="hidden lg:table-cell">Created</TableHead>
                <TableHead className="w-12">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {tickets.map((ticket) => (
                <TableRow
                  key={ticket.id}
                  className="cursor-pointer"
                  onClick={() => handleRowClick(ticket)}
                >
                  <TableCell className="font-medium max-w-xs truncate">
                    {ticket.subject}
                  </TableCell>
                  <TableCell>
                    <StatusBadge status={ticket.status} />
                  </TableCell>
                  <TableCell>
                    <PriorityBadge priority={ticket.priority} />
                  </TableCell>
                  <TableCell className="hidden sm:table-cell font-mono text-xs text-muted-foreground">
                    {ticket.customer_id?.slice(0, 8) ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-muted-foreground capitalize">
                    {ticket.channel ?? "-"}
                  </TableCell>
                  <TableCell className="hidden lg:table-cell text-muted-foreground whitespace-nowrap">
                    {new Date(ticket.created_at).toLocaleDateString()}
                  </TableCell>
                  <TableCell>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-8 w-8 text-muted-foreground hover:text-destructive"
                      aria-label={`Delete ticket: ${ticket.subject}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        setDeleteTarget(ticket);
                      }}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}

      {/* Pagination */}
      {totalPages > 1 && (
        <div className="flex items-center justify-between">
          <p className="text-sm text-muted-foreground">
            Page {page + 1} of {totalPages}
          </p>
          <div className="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPage(page - 1)}
              disabled={!hasPrev}
              aria-label="Previous page"
            >
              <ChevronLeft className="h-4 w-4 mr-1" />
              Previous
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPage(page + 1)}
              disabled={!hasNext}
              aria-label="Next page"
            >
              Next
              <ChevronRight className="h-4 w-4 ml-1" />
            </Button>
          </div>
        </div>
      )}

      {/* Create Ticket Dialog */}
      <Dialog
        open={createOpen}
        onOpenChange={(open) => {
          setCreateOpen(open);
          if (!open) form.reset();
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>New Ticket</DialogTitle>
          </DialogHeader>

          <form
            onSubmit={form.handleSubmit(handleCreate)}
            className="space-y-4"
            noValidate
          >
            {/* Customer ID */}
            <div className="space-y-1.5">
              <Label htmlFor="customer_id">Customer ID</Label>
              <Input
                id="customer_id"
                placeholder="Enter customer UUID"
                {...form.register("customer_id")}
                aria-invalid={!!form.formState.errors.customer_id}
              />
              {form.formState.errors.customer_id && (
                <p className="text-xs text-destructive" role="alert">
                  {form.formState.errors.customer_id.message}
                </p>
              )}
            </div>

            {/* Subject */}
            <div className="space-y-1.5">
              <Label htmlFor="subject">
                Subject <span aria-hidden="true">*</span>
              </Label>
              <Input
                id="subject"
                placeholder="Brief description of the issue"
                {...form.register("subject")}
                aria-invalid={!!form.formState.errors.subject}
              />
              {form.formState.errors.subject && (
                <p className="text-xs text-destructive" role="alert">
                  {form.formState.errors.subject.message}
                </p>
              )}
            </div>

            {/* Description */}
            <div className="space-y-1.5">
              <Label htmlFor="description">Description</Label>
              <Textarea
                id="description"
                placeholder="Detailed description of the issue (optional)"
                rows={3}
                {...form.register("description")}
              />
            </div>

            {/* Priority + Channel row */}
            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label htmlFor="priority">Priority</Label>
                <Select
                  onValueChange={(val) =>
                    form.setValue("priority", val === "_none" ? "" : val)
                  }
                >
                  <SelectTrigger id="priority" aria-label="Select priority">
                    <SelectValue placeholder="Select priority" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="_none">None</SelectItem>
                    {PRIORITY_OPTIONS.map((opt) => (
                      <SelectItem key={opt.value} value={opt.value}>
                        {opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="channel">Channel</Label>
                <Select
                  onValueChange={(val) =>
                    form.setValue("channel", val === "_none" ? "" : val)
                  }
                >
                  <SelectTrigger id="channel" aria-label="Select channel">
                    <SelectValue placeholder="Select channel" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="_none">None</SelectItem>
                    {CHANNEL_OPTIONS.map((opt) => (
                      <SelectItem key={opt.value} value={opt.value}>
                        {opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>

            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={() => setCreateOpen(false)}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={createTicket.isPending}>
                {createTicket.isPending ? "Creating..." : "Create Ticket"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      {/* Delete Confirm Dialog */}
      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title="Delete ticket?"
        description={
          deleteTarget
            ? `"${deleteTarget.subject}" will be permanently deleted. This action cannot be undone.`
            : ""
        }
        onConfirm={handleDelete}
        loading={deleteTicket.isPending}
      />
    </div>
  );
}
