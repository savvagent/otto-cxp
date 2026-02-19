import { useState } from "react";
import { useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { FileText, Pencil, Trash2 } from "lucide-react";

import {
  usePages,
  useCreatePage,
  useUpdatePage,
  useDeletePage,
} from "@/hooks/use-pages";
import { useApplications } from "@/hooks/use-applications";
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
import type { Page } from "@/lib/types";

const PAGE_SIZE = 20;

const pageSchema = z.object({
  application_id: z.string().optional(),
  team_id: z.string().min(1, "Team is required"),
  name: z.string().min(1, "Name is required"),
  path: z.string().optional(),
  description: z.string().optional(),
});

type PageFormValues = z.infer<typeof pageSchema>;

function PageDialog({
  open,
  onOpenChange,
  editing,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: Page | null;
}) {
  const createPage = useCreatePage();
  const updatePage = useUpdatePage();
  const { data: appsData } = useApplications({ limit: 200 });
  const applications = appsData?.data ?? [];
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
  } = useForm<PageFormValues>({
    resolver: zodResolver(pageSchema),
    values: editing
      ? {
          application_id: editing.application_id ?? undefined,
          team_id: editing.team_id ?? "",
          name: editing.name,
          path: editing.path ?? "",
          description: editing.description ?? "",
        }
      : undefined,
  });

  const applicationId = watch("application_id");
  const teamId = watch("team_id");

  function onSubmit(values: PageFormValues) {
    if (isEdit && editing) {
      updatePage.mutate(
        {
          id: editing.id,
          data: {
            name: values.name,
            path: values.path || undefined,
            description: values.description || undefined,
            team_id: values.team_id,
          },
        },
        {
          onSuccess: () => {
            toast.success("Page updated");
            onOpenChange(false);
          },
          onError: (err) => toast.error(`Failed: ${err.message}`),
        }
      );
    } else {
      createPage.mutate(
        {
          application_id: values.application_id || undefined,
          team_id: values.team_id,
          name: values.name,
          path: values.path || undefined,
          description: values.description || undefined,
        },
        {
          onSuccess: () => {
            toast.success("Page created");
            reset();
            onOpenChange(false);
          },
          onError: (err) => toast.error(`Failed: ${err.message}`),
        }
      );
    }
  }

  const isPending = createPage.isPending || updatePage.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{isEdit ? "Edit Page" : "New Page"}</DialogTitle>
        </DialogHeader>
        <form onSubmit={handleSubmit(onSubmit)} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="page-team">
              Team <span className="text-destructive">*</span>
            </Label>
            <Select
              value={teamId || "_none"}
              onValueChange={(v) =>
                setValue("team_id", v === "_none" ? "" : v, { shouldValidate: true })
              }
            >
              <SelectTrigger id="page-team">
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
            <Label htmlFor="page-application">Application</Label>
            <Select
              value={applicationId ?? "_none"}
              onValueChange={(v) =>
                setValue("application_id", v === "_none" ? "" : v)
              }
            >
              <SelectTrigger id="page-application">
                <SelectValue placeholder="None" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="_none">None</SelectItem>
                {applications.map((app) => (
                  <SelectItem key={app.id} value={app.id}>
                    {app.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="page-name">
              Name <span className="text-destructive">*</span>
            </Label>
            <Input
              id="page-name"
              placeholder="Home"
              {...register("name")}
            />
            {errors.name && (
              <p className="text-xs text-destructive">{errors.name.message}</p>
            )}
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="page-path">Path</Label>
            <Input
              id="page-path"
              placeholder="/home"
              {...register("path")}
            />
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="page-desc">Description</Label>
            <Textarea
              id="page-desc"
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

export function PagesPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [page] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<Page | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Page | null>(null);

  const applicationId = searchParams.get("application_id") ?? undefined;

  const { data, isLoading } = usePages({
    application_id: applicationId,
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  });

  const { data: appsData } = useApplications({ limit: 200 });
  const applications = appsData?.data ?? [];

  const { data: teamsData } = useTeams({ limit: 200 });
  const teams = teamsData?.data ?? [];

  const deletePage = useDeletePage();
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
    deletePage.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast.success("Page deleted");
        setDeleteTarget(null);
      },
      onError: (err) => toast.error(`Failed: ${err.message}`),
    });
  }

  function applicationName(id: string) {
    return applications.find((a) => a.id === id)?.name ?? id.slice(0, 8);
  }

  function teamName(id: string) {
    return teams.find((t) => t.id === id)?.name ?? id.slice(0, 8);
  }

  return (
    <div className="space-y-6">
      <PageHeader
        title="Pages"
        action="New Page"
        onAction={() => setCreateOpen(true)}
      />

      {/* Filter row */}
      <div className="flex flex-wrap items-center gap-3">
        <Select
          value={applicationId ?? "all"}
          onValueChange={(v) =>
            setFilter("application_id", v === "all" ? undefined : v)
          }
        >
          <SelectTrigger className="w-[200px]" aria-label="Filter by application">
            <SelectValue placeholder="All applications" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All applications</SelectItem>
            {applications.map((app) => (
              <SelectItem key={app.id} value={app.id}>
                {app.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {applicationId && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setFilter("application_id", undefined)}
          >
            Clear filter
          </Button>
        )}
      </div>

      {isLoading ? (
        <LoadingTable />
      ) : items.length === 0 ? (
        <EmptyState
          icon={FileText}
          title="No pages yet"
          description="Create your first page to add it to the catalog."
          action="New Page"
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
                <TableHead className="hidden sm:table-cell">Path</TableHead>
                <TableHead className="hidden md:table-cell">Application</TableHead>
                <TableHead className="hidden lg:table-cell">Description</TableHead>
                <TableHead className="hidden md:table-cell">Created At</TableHead>
                <TableHead className="w-[100px]" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((pg) => (
                <TableRow key={pg.id}>
                  <TableCell><CopyableId value={pg.id} /></TableCell>
                  <TableCell className="font-medium">{pg.name}</TableCell>
                  <TableCell className="hidden sm:table-cell text-sm">
                    {pg.team_id ? teamName(pg.team_id) : "—"}
                  </TableCell>
                  <TableCell className="hidden sm:table-cell text-sm font-mono text-muted-foreground">
                    {pg.path ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm">
                    {pg.application_id ? applicationName(pg.application_id) : "—"}
                  </TableCell>
                  <TableCell className="hidden lg:table-cell max-w-[200px] truncate text-muted-foreground text-sm">
                    {pg.description ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                    {new Date(pg.created_at).toLocaleDateString()}
                  </TableCell>
                  <TableCell>
                    <div className="flex items-center gap-1">
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Edit page"
                        onClick={() => setEditTarget(pg)}
                      >
                        <Pencil className="h-4 w-4 text-muted-foreground" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Delete page"
                        onClick={() => setDeleteTarget(pg)}
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
          Showing {Math.min(PAGE_SIZE, items.length)} of {total} pages
        </p>
      )}

      <PageDialog
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
        title="Delete Page"
        description={`Are you sure you want to delete "${deleteTarget?.name}"? This cannot be undone.`}
        onConfirm={handleDelete}
        loading={deletePage.isPending}
      />
    </div>
  );
}
