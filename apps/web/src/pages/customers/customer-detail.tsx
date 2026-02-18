/**
 * CustomerDetailPage
 *
 * Usage: <Route path="customers/:id" element={<CustomerDetailPage />} />
 *
 * Features:
 * - Profile card (name, email, phone, company, tags, created date)
 * - Edit Customer dialog with pre-filled react-hook-form
 * - Three tabs: Tickets | Feedback | Interactions
 * - Skeleton loading state
 * - Back link to /customers
 */

import { useState } from "react";
import { useParams, Link } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import {
  ArrowLeft,
  Mail,
  Phone,
  Building2,
  Calendar,
  Pencil,
  MessageSquare,
  Star,
  Zap,
  UserCircle2,
} from "lucide-react";

import {
  useCustomer,
  useUpdateCustomer,
} from "@/hooks/use-customers";
import { customersApi } from "@/lib/api";
import type { Customer, Customer360, Ticket, Feedback, Interaction } from "@/lib/types";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
  DialogDescription,
} from "@/components/ui/dialog";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

import { StatusBadge } from "@/components/shared/status-badge";
import { PriorityBadge } from "@/components/shared/priority-badge";
import { EmptyState } from "@/components/shared/empty-state";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatDate(iso: string) {
  return new Date(iso).toLocaleDateString("en-US", {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

function formatDateTime(iso: string) {
  return new Date(iso).toLocaleString("en-US", {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function displayName(customer: Customer): string {
  return customer.name ?? customer.email;
}

// ---------------------------------------------------------------------------
// Edit Customer Dialog
// ---------------------------------------------------------------------------

const editCustomerSchema = z.object({
  email: z.string().email("Enter a valid email address"),
  name: z.string().optional(),
  phone: z.string().optional(),
  company: z.string().optional(),
  tags: z.string().optional(),
});

type EditCustomerForm = z.infer<typeof editCustomerSchema>;

function EditCustomerDialog({
  customer,
  open,
  onOpenChange,
}: {
  customer: Customer;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const updateCustomer = useUpdateCustomer();

  const form = useForm<EditCustomerForm>({
    resolver: zodResolver(editCustomerSchema),
    values: {
      email: customer.email,
      name: customer.name ?? "",
      phone: customer.phone ?? "",
      company: customer.company ?? "",
      tags: customer.tags.join(", "),
    },
  });

  function handleClose() {
    form.reset();
    onOpenChange(false);
  }

  async function onSubmit(values: EditCustomerForm) {
    const tags = values.tags
      ? values.tags
          .split(",")
          .map((t) => t.trim())
          .filter(Boolean)
      : [];

    try {
      await updateCustomer.mutateAsync({
        id: customer.id,
        data: {
          email: values.email,
          name: values.name || undefined,
          phone: values.phone || undefined,
          company: values.company || undefined,
          tags,
        },
      });
      toast.success("Customer updated");
      handleClose();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Failed to update customer");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-[480px]">
        <DialogHeader>
          <DialogTitle>Edit Customer</DialogTitle>
          <DialogDescription>
            Update profile details for {displayName(customer)}.
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-4">
          {/* Email */}
          <div className="space-y-1.5">
            <Label htmlFor="edit-email">
              Email <span aria-hidden="true" className="text-destructive">*</span>
            </Label>
            <Input
              id="edit-email"
              type="email"
              autoComplete="email"
              aria-required="true"
              aria-describedby={
                form.formState.errors.email ? "edit-email-error" : undefined
              }
              {...form.register("email")}
            />
            {form.formState.errors.email && (
              <p id="edit-email-error" role="alert" className="text-xs text-destructive">
                {form.formState.errors.email.message}
              </p>
            )}
          </div>

          {/* Name */}
          <div className="space-y-1.5">
            <Label htmlFor="edit-name">Name</Label>
            <Input id="edit-name" autoComplete="name" {...form.register("name")} />
          </div>

          {/* Phone */}
          <div className="space-y-1.5">
            <Label htmlFor="edit-phone">Phone</Label>
            <Input
              id="edit-phone"
              type="tel"
              autoComplete="tel"
              {...form.register("phone")}
            />
          </div>

          {/* Company */}
          <div className="space-y-1.5">
            <Label htmlFor="edit-company">Company</Label>
            <Input
              id="edit-company"
              autoComplete="organization"
              {...form.register("company")}
            />
          </div>

          {/* Tags */}
          <div className="space-y-1.5">
            <Label htmlFor="edit-tags">Tags</Label>
            <Input
              id="edit-tags"
              placeholder="vip, enterprise (comma separated)"
              {...form.register("tags")}
            />
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={handleClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={updateCustomer.isPending}>
              {updateCustomer.isPending ? "Saving..." : "Save Changes"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Profile card skeleton
// ---------------------------------------------------------------------------

function ProfileSkeleton() {
  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-4">
        <div className="flex items-center gap-4">
          <Skeleton className="h-14 w-14 rounded-full" />
          <div className="space-y-2">
            <Skeleton className="h-5 w-40" />
            <Skeleton className="h-4 w-56" />
          </div>
        </div>
        <Skeleton className="h-9 w-20" />
      </CardHeader>
      <CardContent className="space-y-3">
        <Skeleton className="h-4 w-48" />
        <Skeleton className="h-4 w-36" />
        <Skeleton className="h-4 w-32" />
        <div className="flex gap-2 pt-1">
          <Skeleton className="h-5 w-12 rounded-full" />
          <Skeleton className="h-5 w-16 rounded-full" />
        </div>
      </CardContent>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Profile card
// ---------------------------------------------------------------------------

function ProfileCard({
  customer,
  onEdit,
}: {
  customer: Customer;
  onEdit: () => void;
}) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-4">
        <div className="flex items-center gap-4">
          <div
            className="flex h-14 w-14 shrink-0 items-center justify-center rounded-full bg-primary/10 text-primary"
            aria-hidden="true"
          >
            <UserCircle2 className="h-8 w-8" />
          </div>
          <div>
            <CardTitle className="text-xl">
              {customer.name ?? (
                <span className="text-muted-foreground font-normal italic">
                  No name
                </span>
              )}
            </CardTitle>
            <p className="text-sm text-muted-foreground mt-0.5">{customer.email}</p>
          </div>
        </div>
        <Button variant="outline" size="sm" onClick={onEdit}>
          <Pencil className="mr-2 h-3.5 w-3.5" />
          Edit
        </Button>
      </CardHeader>

      <CardContent>
        <dl className="space-y-2 text-sm">
          <div className="flex items-center gap-2 text-muted-foreground">
            <Mail className="h-4 w-4 shrink-0" aria-hidden="true" />
            <a
              href={`mailto:${customer.email}`}
              className="hover:underline hover:text-foreground"
            >
              {customer.email}
            </a>
          </div>

          {customer.phone && (
            <div className="flex items-center gap-2 text-muted-foreground">
              <Phone className="h-4 w-4 shrink-0" aria-hidden="true" />
              <a
                href={`tel:${customer.phone}`}
                className="hover:underline hover:text-foreground"
              >
                {customer.phone}
              </a>
            </div>
          )}

          {customer.company && (
            <div className="flex items-center gap-2 text-muted-foreground">
              <Building2 className="h-4 w-4 shrink-0" aria-hidden="true" />
              <span>{customer.company}</span>
            </div>
          )}

          <div className="flex items-center gap-2 text-muted-foreground">
            <Calendar className="h-4 w-4 shrink-0" aria-hidden="true" />
            <span>Customer since {formatDate(customer.created_at)}</span>
          </div>
        </dl>

        {customer.tags.length > 0 && (
          <div className="flex flex-wrap gap-1.5 mt-4" aria-label="Tags">
            {customer.tags.map((tag) => (
              <Badge key={tag} variant="secondary" className="text-xs">
                {tag}
              </Badge>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Tickets tab
// ---------------------------------------------------------------------------

function TicketsTab({ tickets }: { tickets: Ticket[] }) {
  if (tickets.length === 0) {
    return (
      <EmptyState
        icon={MessageSquare}
        title="No tickets"
        description="This customer has no support tickets yet."
      />
    );
  }

  return (
    <div className="rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Subject</TableHead>
            <TableHead>Status</TableHead>
            <TableHead className="hidden sm:table-cell">Priority</TableHead>
            <TableHead className="hidden md:table-cell">Channel</TableHead>
            <TableHead className="hidden sm:table-cell">Created</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {tickets.map((ticket) => (
            <TableRow key={ticket.id}>
              <TableCell className="font-medium max-w-[200px] truncate">
                {ticket.subject}
              </TableCell>
              <TableCell>
                <StatusBadge status={ticket.status} />
              </TableCell>
              <TableCell className="hidden sm:table-cell">
                <PriorityBadge priority={ticket.priority} />
              </TableCell>
              <TableCell className="hidden md:table-cell text-muted-foreground text-sm">
                {ticket.channel ?? "—"}
              </TableCell>
              <TableCell className="hidden sm:table-cell text-muted-foreground text-sm">
                {formatDate(ticket.created_at)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Feedback tab
// ---------------------------------------------------------------------------

function FeedbackTab({ feedback }: { feedback: Feedback[] }) {
  if (feedback.length === 0) {
    return (
      <EmptyState
        icon={Star}
        title="No feedback"
        description="This customer has not submitted any feedback yet."
      />
    );
  }

  return (
    <div className="rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Type</TableHead>
            <TableHead>Subject</TableHead>
            <TableHead className="hidden sm:table-cell">Rating</TableHead>
            <TableHead className="hidden md:table-cell">Date</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {feedback.map((item) => (
            <TableRow key={item.id}>
              <TableCell>
                <Badge variant="outline" className="capitalize text-xs">
                  {item.feedback_type.replace(/_/g, " ")}
                </Badge>
              </TableCell>
              <TableCell className="max-w-[220px]">
                <p className="truncate font-medium">
                  {item.subject ?? (
                    <span className="text-muted-foreground italic">No subject</span>
                  )}
                </p>
                {item.body && (
                  <p className="text-xs text-muted-foreground truncate mt-0.5">
                    {item.body}
                  </p>
                )}
              </TableCell>
              <TableCell className="hidden sm:table-cell text-muted-foreground">
                {item.rating != null ? (
                  <span className="flex items-center gap-1">
                    <Star className="h-3.5 w-3.5 fill-yellow-400 text-yellow-400" />
                    {item.rating}
                  </span>
                ) : (
                  "—"
                )}
              </TableCell>
              <TableCell className="hidden md:table-cell text-muted-foreground text-sm">
                {formatDate(item.created_at)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Interactions tab
// ---------------------------------------------------------------------------

function InteractionsTab({ interactions }: { interactions: Interaction[] }) {
  if (interactions.length === 0) {
    return (
      <EmptyState
        icon={Zap}
        title="No interactions"
        description="No interactions have been recorded for this customer."
      />
    );
  }

  return (
    <div className="rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Type</TableHead>
            <TableHead className="hidden sm:table-cell">Date</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {interactions.map((interaction) => (
            <TableRow key={interaction.id}>
              <TableCell>
                <Badge variant="outline" className="capitalize text-xs">
                  {interaction.interaction_type.replace(/_/g, " ")}
                </Badge>
              </TableCell>
              <TableCell className="hidden sm:table-cell text-muted-foreground text-sm">
                {formatDateTime(interaction.created_at)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export function CustomerDetailPage() {
  const { id } = useParams<{ id: string }>();
  const [editOpen, setEditOpen] = useState(false);

  const {
    data: customer,
    isLoading: customerLoading,
    isError: customerError,
  } = useCustomer(id);

  const {
    data: data360,
    isLoading: loading360,
  } = useQuery<Customer360>({
    queryKey: ["customers", id, "360"],
    queryFn: () => customersApi.get360(id!),
    enabled: !!id,
  });

  const isLoading = customerLoading || loading360;

  // ---------------------------------------------------------------------------
  // Loading state
  // ---------------------------------------------------------------------------
  if (isLoading) {
    return (
      <div className="flex flex-col gap-6">
        {/* Back link skeleton */}
        <Skeleton className="h-4 w-28" />

        <ProfileSkeleton />

        <div className="space-y-4">
          <Skeleton className="h-10 w-64" />
          <Skeleton className="h-48 w-full" />
        </div>
      </div>
    );
  }

  // ---------------------------------------------------------------------------
  // Error / not found state
  // ---------------------------------------------------------------------------
  if (customerError || !customer) {
    return (
      <div className="flex flex-col gap-6">
        <Link
          to="/customers"
          className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground w-fit"
        >
          <ArrowLeft className="h-4 w-4" />
          Back to Customers
        </Link>
        <EmptyState
          title="Customer not found"
          description="This customer may have been deleted or the link is invalid."
          action="View all customers"
          onAction={() => window.location.assign("/customers")}
        />
      </div>
    );
  }

  const tickets = data360?.recent_tickets ?? [];
  const feedback = data360?.recent_feedback ?? [];
  const interactions = data360?.recent_interactions ?? [];

  return (
    <div className="flex flex-col gap-6">
      {/* Back navigation */}
      <Link
        to="/customers"
        className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground w-fit transition-colors"
      >
        <ArrowLeft className="h-4 w-4" />
        Back to Customers
      </Link>

      {/* Profile card */}
      <ProfileCard customer={customer} onEdit={() => setEditOpen(true)} />

      {/* Activity tabs */}
      <Tabs defaultValue="tickets">
        <TabsList aria-label="Customer activity">
          <TabsTrigger value="tickets">
            Tickets
            {tickets.length > 0 && (
              <Badge
                variant="secondary"
                className="ml-2 h-5 min-w-[20px] px-1 text-xs"
                aria-label={`${tickets.length} tickets`}
              >
                {tickets.length}
              </Badge>
            )}
          </TabsTrigger>
          <TabsTrigger value="feedback">
            Feedback
            {feedback.length > 0 && (
              <Badge
                variant="secondary"
                className="ml-2 h-5 min-w-[20px] px-1 text-xs"
                aria-label={`${feedback.length} feedback items`}
              >
                {feedback.length}
              </Badge>
            )}
          </TabsTrigger>
          <TabsTrigger value="interactions">
            Interactions
            {interactions.length > 0 && (
              <Badge
                variant="secondary"
                className="ml-2 h-5 min-w-[20px] px-1 text-xs"
                aria-label={`${interactions.length} interactions`}
              >
                {interactions.length}
              </Badge>
            )}
          </TabsTrigger>
        </TabsList>

        <TabsContent value="tickets" className="mt-4">
          <TicketsTab tickets={tickets} />
        </TabsContent>

        <TabsContent value="feedback" className="mt-4">
          <FeedbackTab feedback={feedback} />
        </TabsContent>

        <TabsContent value="interactions" className="mt-4">
          <InteractionsTab interactions={interactions} />
        </TabsContent>
      </Tabs>

      {/* Edit dialog (only render when customer is available) */}
      <EditCustomerDialog
        customer={customer}
        open={editOpen}
        onOpenChange={setEditOpen}
      />
    </div>
  );
}
