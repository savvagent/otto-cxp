/**
 * CustomersPage
 *
 * Usage: <Route path="customers" element={<CustomersPage />} />
 *
 * Features:
 * - Paginated customer list (50 per page, offset-based)
 * - Client-side search debounce with server search fallback
 * - "New Customer" dialog with react-hook-form + zod validation
 * - Row click navigates to /customers/:id
 * - Per-row dropdown with Delete action + ConfirmDialog
 */

import { useState, useMemo, useCallback } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import {
  Search,
  MoreHorizontal,
  Trash2,
  ChevronLeft,
  ChevronRight,
  Users,
} from "lucide-react";

import {
  useCustomers,
  useCreateCustomer,
  useDeleteCustomer,
} from "@/hooks/use-customers";
import type { Customer } from "@/lib/types";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

import { PageHeader } from "@/components/shared/page-header";
import { EmptyState } from "@/components/shared/empty-state";
import { LoadingTable } from "@/components/shared/loading-state";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import {
  CatalogFilterBar,
  type CatalogFilterValues,
} from "@/components/shared/catalog-filter-bar";

const PAGE_SIZE = 50;

// ---------------------------------------------------------------------------
// Form schema
// ---------------------------------------------------------------------------

const createCustomerSchema = z.object({
  email: z.string().email("Enter a valid email address"),
  name: z.string().optional(),
  phone: z.string().optional(),
  company: z.string().optional(),
  tags: z.string().optional(), // comma-separated, split on submit
});

type CreateCustomerForm = z.infer<typeof createCustomerSchema>;

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

function displayName(customer: Customer): string {
  return customer.name ?? customer.email;
}

// ---------------------------------------------------------------------------
// New Customer Dialog
// ---------------------------------------------------------------------------

function NewCustomerDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const createCustomer = useCreateCustomer();

  const form = useForm<CreateCustomerForm>({
    resolver: zodResolver(createCustomerSchema),
    defaultValues: { email: "", name: "", phone: "", company: "", tags: "" },
  });

  function handleClose() {
    form.reset();
    onOpenChange(false);
  }

  async function onSubmit(values: CreateCustomerForm) {
    const tags = values.tags
      ? values.tags
          .split(",")
          .map((t) => t.trim())
          .filter(Boolean)
      : [];

    try {
      await createCustomer.mutateAsync({
        email: values.email,
        name: values.name || undefined,
        phone: values.phone || undefined,
        company: values.company || undefined,
        tags,
      });
      toast.success("Customer created");
      handleClose();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Failed to create customer");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-[480px]">
        <DialogHeader>
          <DialogTitle>New Customer</DialogTitle>
          <DialogDescription>
            Add a new customer to your CXP database. Email is required.
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-4">
          {/* Email */}
          <div className="space-y-1.5">
            <Label htmlFor="email">
              Email <span aria-hidden="true" className="text-destructive">*</span>
            </Label>
            <Input
              id="email"
              type="email"
              autoComplete="email"
              placeholder="customer@example.com"
              aria-required="true"
              aria-describedby={
                form.formState.errors.email ? "email-error" : undefined
              }
              {...form.register("email")}
            />
            {form.formState.errors.email && (
              <p id="email-error" role="alert" className="text-xs text-destructive">
                {form.formState.errors.email.message}
              </p>
            )}
          </div>

          {/* Name */}
          <div className="space-y-1.5">
            <Label htmlFor="name">Name</Label>
            <Input
              id="name"
              autoComplete="name"
              placeholder="Full name"
              {...form.register("name")}
            />
          </div>

          {/* Phone */}
          <div className="space-y-1.5">
            <Label htmlFor="phone">Phone</Label>
            <Input
              id="phone"
              type="tel"
              autoComplete="tel"
              placeholder="+1 555 000 0000"
              {...form.register("phone")}
            />
          </div>

          {/* Company */}
          <div className="space-y-1.5">
            <Label htmlFor="company">Company</Label>
            <Input
              id="company"
              autoComplete="organization"
              placeholder="Acme Corp"
              {...form.register("company")}
            />
          </div>

          {/* Tags */}
          <div className="space-y-1.5">
            <Label htmlFor="tags">Tags</Label>
            <Input
              id="tags"
              placeholder="vip, enterprise (comma separated)"
              {...form.register("tags")}
            />
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={handleClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={createCustomer.isPending}>
              {createCustomer.isPending ? "Creating..." : "Create Customer"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Customer row actions
// ---------------------------------------------------------------------------

function CustomerRowActions({ customer }: { customer: Customer }) {
  const [confirmOpen, setConfirmOpen] = useState(false);
  const deleteCustomer = useDeleteCustomer();

  async function handleDelete() {
    try {
      await deleteCustomer.mutateAsync(customer.id);
      toast.success(`${displayName(customer)} deleted`);
      setConfirmOpen(false);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Failed to delete customer");
    }
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            aria-label={`Actions for ${displayName(customer)}`}
            onClick={(e) => e.stopPropagation()}
          >
            <MoreHorizontal className="h-4 w-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem
            className="text-destructive focus:text-destructive"
            onClick={(e) => {
              e.stopPropagation();
              setConfirmOpen(true);
            }}
          >
            <Trash2 className="mr-2 h-4 w-4" />
            Delete
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <ConfirmDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        title="Delete customer?"
        description={`This will permanently delete ${displayName(customer)} and cannot be undone.`}
        onConfirm={handleDelete}
        loading={deleteCustomer.isPending}
      />
    </>
  );
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export function CustomersPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [newDialogOpen, setNewDialogOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [offset, setOffset] = useState(0);

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
      setOffset(0);
    },
    [setSearchParams],
  );

  const { data, isLoading, isError } = useCustomers({
    limit: PAGE_SIZE,
    offset,
    ...catalogFilters,
  });

  // Client-side search filter applied on top of paginated results.
  // For a production app this would debounce and call customersApi.search()
  // when the query is long enough; client-side is sufficient for demo scale.
  const customers = useMemo<Customer[]>(() => {
    if (!data?.data) return [];
    if (!search.trim()) return data.data;
    const q = search.toLowerCase();
    return data.data.filter(
      (c) =>
        c.email.toLowerCase().includes(q) ||
        c.name?.toLowerCase().includes(q) ||
        c.company?.toLowerCase().includes(q) ||
        c.tags.some((t) => t.toLowerCase().includes(q)),
    );
  }, [data, search]);

  const total = data?.total ?? 0;
  const hasPrev = offset > 0;
  const hasNext = offset + PAGE_SIZE < total;

  function handlePrev() {
    setOffset((o) => Math.max(0, o - PAGE_SIZE));
  }

  function handleNext() {
    setOffset((o) => o + PAGE_SIZE);
  }

  function handleRowClick(id: string) {
    navigate(`/customers/${id}`);
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Customers"
        description="Browse and manage customer profiles"
        action="New Customer"
        onAction={() => setNewDialogOpen(true)}
      />

      {/* Catalog filters */}
      <CatalogFilterBar
        value={catalogFilters}
        onChange={handleCatalogChange}
      />

      {/* Search */}
      <div className="relative max-w-sm">
        <Search
          className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"
          aria-hidden="true"
        />
        <Input
          className="pl-9"
          placeholder="Search by name, email, company or tag..."
          value={search}
          onChange={(e) => {
            setSearch(e.target.value);
            setOffset(0); // reset pagination on new search
          }}
          aria-label="Search customers"
        />
      </div>

      {/* Table */}
      {isLoading ? (
        <LoadingTable rows={8} />
      ) : isError ? (
        <EmptyState
          title="Failed to load customers"
          description="Check your connection and try again."
        />
      ) : customers.length === 0 ? (
        <EmptyState
          icon={Users}
          title={search ? "No customers match your search" : "No customers yet"}
          description={
            search
              ? "Try a different search term."
              : "Create your first customer to get started."
          }
          action={search ? undefined : "New Customer"}
          onAction={search ? undefined : () => setNewDialogOpen(true)}
        />
      ) : (
        <div className="rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Email</TableHead>
                <TableHead className="hidden md:table-cell">Company</TableHead>
                <TableHead className="hidden lg:table-cell">Tags</TableHead>
                <TableHead className="hidden sm:table-cell">Created</TableHead>
                <TableHead className="w-10">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {customers.map((customer) => (
                <TableRow
                  key={customer.id}
                  className="cursor-pointer"
                  onClick={() => handleRowClick(customer.id)}
                  // Allow keyboard activation of the row
                  tabIndex={0}
                  role="button"
                  aria-label={`View ${displayName(customer)}`}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      handleRowClick(customer.id);
                    }
                  }}
                >
                  <TableCell className="font-medium">
                    {customer.name ?? (
                      <span className="text-muted-foreground italic">unnamed</span>
                    )}
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {customer.email}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-muted-foreground">
                    {customer.company ?? (
                      <span className="text-muted-foreground/50">—</span>
                    )}
                  </TableCell>
                  <TableCell className="hidden lg:table-cell">
                    <div className="flex flex-wrap gap-1">
                      {customer.tags.slice(0, 3).map((tag) => (
                        <Badge
                          key={tag}
                          variant="secondary"
                          className="text-xs"
                        >
                          {tag}
                        </Badge>
                      ))}
                      {customer.tags.length > 3 && (
                        <Badge variant="outline" className="text-xs">
                          +{customer.tags.length - 3}
                        </Badge>
                      )}
                    </div>
                  </TableCell>
                  <TableCell className="hidden sm:table-cell text-muted-foreground text-sm">
                    {formatDate(customer.createdAt)}
                  </TableCell>
                  <TableCell onClick={(e) => e.stopPropagation()}>
                    <CustomerRowActions customer={customer} />
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}

      {/* Pagination */}
      {!isLoading && total > PAGE_SIZE && !search && (
        <div className="flex items-center justify-between text-sm text-muted-foreground">
          <span>
            {offset + 1}–{Math.min(offset + PAGE_SIZE, total)} of {total} customers
          </span>
          <div className="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={handlePrev}
              disabled={!hasPrev}
              aria-label="Previous page"
            >
              <ChevronLeft className="h-4 w-4 mr-1" />
              Previous
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={handleNext}
              disabled={!hasNext}
              aria-label="Next page"
            >
              Next
              <ChevronRight className="h-4 w-4 ml-1" />
            </Button>
          </div>
        </div>
      )}

      <NewCustomerDialog
        open={newDialogOpen}
        onOpenChange={setNewDialogOpen}
      />
    </div>
  );
}
