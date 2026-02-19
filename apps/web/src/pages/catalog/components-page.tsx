import { useState } from "react";
import { useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Puzzle, Pencil, Trash2 } from "lucide-react";

import {
  useComponents,
  useCreateComponent,
  useUpdateComponent,
  useDeleteComponent,
} from "@/hooks/use-components";
import { usePages } from "@/hooks/use-pages";
import { useTeams } from "@/hooks/use-teams";
import { PageHeader } from "@/components/shared/page-header";
import { CopyableId } from "@/components/shared/copyable-id";
import { LoadingTable } from "@/components/shared/loading-state";
import { EmptyState } from "@/components/shared/empty-state";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
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
import type { Component } from "@/lib/types";

const PAGE_SIZE = 20;

const componentSchema = z.object({
  page_id: z.string().optional(),
  team_id: z.string().min(1, "Team is required"),
  name: z.string().min(1, "Name is required"),
  component_type: z.string().optional(),
  description: z.string().optional(),
});

type ComponentForm = z.infer<typeof componentSchema>;

function ComponentDialog({
  open,
  onOpenChange,
  editing,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: Component | null;
}) {
  const createComponent = useCreateComponent();
  const updateComponent = useUpdateComponent();
  const { data: pagesData } = usePages({ limit: 200 });
  const pages = pagesData?.data ?? [];
  const { data: teamsData } = useTeams({ limit: 200 });
  const teams = teamsData?.data ?? [];
  const isEdit = !!editing;

  const {
    register,
    handleSubmit,
    reset,
    setValue,
    watch,
    formState: { errors },
  } = useForm<ComponentForm>({
    resolver: zodResolver(componentSchema),
    values: editing
      ? {
          page_id: editing.page_id ?? undefined,
          team_id: editing.team_id ?? "",
          name: editing.name,
          component_type: editing.component_type ?? "",
          description: editing.description ?? "",
        }
      : undefined,
  });

  const pageId = watch("page_id");
  const teamId = watch("team_id");

  function onSubmit(values: ComponentForm) {
    if (isEdit && editing) {
      updateComponent.mutate(
        {
          id: editing.id,
          data: {
            name: values.name,
            component_type: values.component_type || undefined,
            description: values.description || undefined,
            team_id: values.team_id,
          },
        },
        {
          onSuccess: () => {
            toast.success("Component updated");
            onOpenChange(false);
          },
          onError: (err) => toast.error(`Failed: ${err.message}`),
        }
      );
    } else {
      createComponent.mutate(
        {
          page_id: values.page_id || undefined,
          team_id: values.team_id,
          name: values.name,
          component_type: values.component_type || undefined,
          description: values.description || undefined,
        },
        {
          onSuccess: () => {
            toast.success("Component created");
            reset();
            onOpenChange(false);
          },
          onError: (err) => toast.error(`Failed: ${err.message}`),
        }
      );
    }
  }

  const isPending = createComponent.isPending || updateComponent.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            {isEdit ? "Edit Component" : "New Component"}
          </DialogTitle>
        </DialogHeader>
        <form onSubmit={handleSubmit(onSubmit)} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="comp-team">
              Team <span className="text-destructive">*</span>
            </Label>
            <Select
              value={teamId || "_none"}
              onValueChange={(v) =>
                setValue("team_id", v === "_none" ? "" : v, { shouldValidate: true })
              }
            >
              <SelectTrigger id="comp-team">
                <SelectValue placeholder="Select a team" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="_none" disabled>Select a team</SelectItem>
                {teams.map((t) => (
                  <SelectItem key={t.id} value={t.id}>
                    {t.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {errors.team_id && (
              <p className="text-xs text-destructive">{errors.team_id.message}</p>
            )}
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="comp-page">Page</Label>
            <Select
              value={pageId ?? "_none"}
              onValueChange={(v) =>
                setValue("page_id", v === "_none" ? "" : v)
              }
            >
              <SelectTrigger id="comp-page">
                <SelectValue placeholder="None" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="_none">None</SelectItem>
                {pages.map((pg) => (
                  <SelectItem key={pg.id} value={pg.id}>
                    {pg.name}
                    {pg.path && (
                      <span className="text-muted-foreground ml-1 font-mono text-xs">
                        {pg.path}
                      </span>
                    )}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="comp-name">
              Name <span className="text-destructive">*</span>
            </Label>
            <Input
              id="comp-name"
              placeholder="Hero Banner"
              {...register("name")}
            />
            {errors.name && (
              <p className="text-xs text-destructive">{errors.name.message}</p>
            )}
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="comp-type">Component Type</Label>
            <Input
              id="comp-type"
              placeholder="button / form / modal / banner"
              {...register("component_type")}
            />
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="comp-desc">Description</Label>
            <Textarea
              id="comp-desc"
              placeholder="Optional description..."
              rows={3}
              {...register("description")}
            />
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={isPending}>
              {isPending ? "Saving..." : isEdit ? "Save Changes" : "Create"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function ComponentsPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [currentPage] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<Component | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Component | null>(null);

  const pageId = searchParams.get("page_id") ?? undefined;

  const { data, isLoading } = useComponents({
    page_id: pageId,
    limit: PAGE_SIZE,
    offset: currentPage * PAGE_SIZE,
  });

  const { data: pagesData } = usePages({ limit: 200 });
  const pages = pagesData?.data ?? [];

  const { data: teamsData } = useTeams({ limit: 200 });
  const teams = teamsData?.data ?? [];

  const deleteComponent = useDeleteComponent();
  const items = data?.data ?? [];
  const total = data?.total ?? 0;

  function setFilter(key: string, value: string | undefined) {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (value) next.set(key, value);
      else next.delete(key);
      return next;
    });
  }

  function handleDelete() {
    if (!deleteTarget) return;
    deleteComponent.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast.success("Component deleted");
        setDeleteTarget(null);
      },
      onError: (err) => toast.error(`Failed: ${err.message}`),
    });
  }

  function pageName(id: string) {
    const pg = pages.find((p) => p.id === id);
    return pg ? `${pg.name}${pg.path ? ` (${pg.path})` : ""}` : id.slice(0, 8);
  }

  function teamName(id: string) {
    return teams.find((t) => t.id === id)?.name ?? id.slice(0, 8);
  }

  return (
    <div className="space-y-6">
      <PageHeader
        title="Components"
        action="New Component"
        onAction={() => setCreateOpen(true)}
      />

      {/* Filter row */}
      <div className="flex flex-wrap items-center gap-3">
        <Select
          value={pageId ?? "all"}
          onValueChange={(v) =>
            setFilter("page_id", v === "all" ? undefined : v)
          }
        >
          <SelectTrigger className="w-[220px]" aria-label="Filter by page">
            <SelectValue placeholder="All pages" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All pages</SelectItem>
            {pages.map((pg) => (
              <SelectItem key={pg.id} value={pg.id}>
                {pg.name}
                {pg.path && (
                  <span className="text-muted-foreground ml-1 font-mono text-xs">
                    {pg.path}
                  </span>
                )}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {pageId && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setFilter("page_id", undefined)}
          >
            Clear filter
          </Button>
        )}
      </div>

      {isLoading ? (
        <LoadingTable />
      ) : items.length === 0 ? (
        <EmptyState
          icon={Puzzle}
          title="No components yet"
          description="Create your first component to add it to the catalog."
          action="New Component"
          onAction={() => setCreateOpen(true)}
        />
      ) : (
        <div className="rounded-md border overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-[100px]">ID</TableHead>
                <TableHead>Name</TableHead>
                <TableHead className="hidden sm:table-cell">Team</TableHead>
                <TableHead className="hidden sm:table-cell">Component Type</TableHead>
                <TableHead className="hidden md:table-cell">Page</TableHead>
                <TableHead className="hidden lg:table-cell">Description</TableHead>
                <TableHead className="hidden md:table-cell">Created At</TableHead>
                <TableHead className="w-[100px]" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((comp) => (
                <TableRow key={comp.id}>
                  <TableCell><CopyableId value={comp.id} /></TableCell>
                  <TableCell className="font-medium">{comp.name}</TableCell>
                  <TableCell className="hidden sm:table-cell text-sm">
                    {comp.team_id ? teamName(comp.team_id) : "—"}
                  </TableCell>
                  <TableCell className="hidden sm:table-cell text-sm text-muted-foreground">
                    {comp.component_type ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm">
                    {comp.page_id ? pageName(comp.page_id) : "—"}
                  </TableCell>
                  <TableCell className="hidden lg:table-cell max-w-[200px] truncate text-muted-foreground text-sm">
                    {comp.description ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                    {new Date(comp.created_at).toLocaleDateString()}
                  </TableCell>
                  <TableCell>
                    <div className="flex items-center gap-1">
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Edit component"
                        onClick={() => setEditTarget(comp)}
                      >
                        <Pencil className="h-4 w-4 text-muted-foreground" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Delete component"
                        onClick={() => setDeleteTarget(comp)}
                      >
                        <Trash2 className="h-4 w-4 text-muted-foreground" />
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}

      {total > PAGE_SIZE && (
        <p className="text-sm text-muted-foreground">
          Showing {Math.min(PAGE_SIZE, items.length)} of {total} components
        </p>
      )}

      <ComponentDialog
        open={createOpen || !!editTarget}
        onOpenChange={(open) => {
          if (!open) {
            setCreateOpen(false);
            setEditTarget(null);
          }
        }}
        editing={editTarget}
      />

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title="Delete Component"
        description={`Are you sure you want to delete "${deleteTarget?.name}"? This cannot be undone.`}
        onConfirm={handleDelete}
        loading={deleteComponent.isPending}
      />
    </div>
  );
}
