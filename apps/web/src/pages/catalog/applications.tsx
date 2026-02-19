import { useState } from "react";
import { useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { AppWindow, Pencil, Trash2 } from "lucide-react";

import {
  useApplications,
  useCreateApplication,
  useUpdateApplication,
  useDeleteApplication,
} from "@/hooks/use-applications";
import { useExperiences } from "@/hooks/use-experiences";
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
import type { Application } from "@/lib/types";

const PAGE_SIZE = 20;

const applicationSchema = z.object({
  experience_id: z.string().optional(),
  team_id: z.string().min(1, "Team is required"),
  name: z.string().min(1, "Name is required"),
  description: z.string().optional(),
  platform: z.string().optional(),
});

type ApplicationForm = z.infer<typeof applicationSchema>;

function ApplicationDialog({
  open,
  onOpenChange,
  editing,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: Application | null;
}) {
  const createApplication = useCreateApplication();
  const updateApplication = useUpdateApplication();
  const { data: experiencesData } = useExperiences({ limit: 200 });
  const experiences = experiencesData?.data ?? [];
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
  } = useForm<ApplicationForm>({
    resolver: zodResolver(applicationSchema),
    values: editing
      ? {
          experience_id: editing.experience_id ?? undefined,
          team_id: editing.team_id ?? "",
          name: editing.name,
          description: editing.description ?? "",
          platform: editing.platform ?? "",
        }
      : undefined,
  });

  const experienceId = watch("experience_id");
  const teamId = watch("team_id");

  function onSubmit(values: ApplicationForm) {
    if (isEdit && editing) {
      updateApplication.mutate(
        {
          id: editing.id,
          data: {
            name: values.name,
            description: values.description || undefined,
            platform: values.platform || undefined,
            team_id: values.team_id,
          },
        },
        {
          onSuccess: () => {
            toast.success("Application updated");
            onOpenChange(false);
          },
          onError: (err) => toast.error(`Failed: ${err.message}`),
        }
      );
    } else {
      createApplication.mutate(
        {
          experience_id: values.experience_id || undefined,
          team_id: values.team_id,
          name: values.name,
          description: values.description || undefined,
          platform: values.platform || undefined,
        },
        {
          onSuccess: () => {
            toast.success("Application created");
            reset();
            onOpenChange(false);
          },
          onError: (err) => toast.error(`Failed: ${err.message}`),
        }
      );
    }
  }

  const isPending = createApplication.isPending || updateApplication.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            {isEdit ? "Edit Application" : "New Application"}
          </DialogTitle>
        </DialogHeader>
        <form onSubmit={handleSubmit(onSubmit)} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="app-team">
              Team <span className="text-destructive">*</span>
            </Label>
            <Select
              value={teamId || "_none"}
              onValueChange={(v) =>
                setValue("team_id", v === "_none" ? "" : v, { shouldValidate: true })
              }
            >
              <SelectTrigger id="app-team">
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
            <Label htmlFor="app-experience">Experience</Label>
            <Select
              value={experienceId ?? "_none"}
              onValueChange={(v) =>
                setValue("experience_id", v === "_none" ? "" : v)
              }
            >
              <SelectTrigger id="app-experience">
                <SelectValue placeholder="None" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="_none">None</SelectItem>
                {experiences.map((exp) => (
                  <SelectItem key={exp.id} value={exp.id}>
                    {exp.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="app-name">
              Name <span className="text-destructive">*</span>
            </Label>
            <Input
              id="app-name"
              placeholder="iOS App"
              {...register("name")}
            />
            {errors.name && (
              <p className="text-xs text-destructive">{errors.name.message}</p>
            )}
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="app-platform">Platform</Label>
            <Input
              id="app-platform"
              placeholder="ios / android / web"
              {...register("platform")}
            />
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="app-desc">Description</Label>
            <Textarea
              id="app-desc"
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

export function ApplicationsPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [page] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<Application | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Application | null>(null);

  const experienceId = searchParams.get("experience_id") ?? undefined;

  const { data, isLoading } = useApplications({
    experience_id: experienceId,
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  });

  const { data: experiencesData } = useExperiences({ limit: 200 });
  const experiences = experiencesData?.data ?? [];

  const { data: teamsData } = useTeams({ limit: 200 });
  const teams = teamsData?.data ?? [];

  const deleteApplication = useDeleteApplication();
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
    deleteApplication.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast.success("Application deleted");
        setDeleteTarget(null);
      },
      onError: (err) => toast.error(`Failed: ${err.message}`),
    });
  }

  function experienceName(id: string) {
    return experiences.find((e) => e.id === id)?.name ?? id.slice(0, 8);
  }

  function teamName(id: string) {
    return teams.find((t) => t.id === id)?.name ?? id.slice(0, 8);
  }

  return (
    <div className="space-y-6">
      <PageHeader
        title="Applications"
        action="New Application"
        onAction={() => setCreateOpen(true)}
      />

      {/* Filter row */}
      <div className="flex flex-wrap items-center gap-3">
        <Select
          value={experienceId ?? "all"}
          onValueChange={(v) =>
            setFilter("experience_id", v === "all" ? undefined : v)
          }
        >
          <SelectTrigger className="w-[200px]" aria-label="Filter by experience">
            <SelectValue placeholder="All experiences" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All experiences</SelectItem>
            {experiences.map((exp) => (
              <SelectItem key={exp.id} value={exp.id}>
                {exp.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {experienceId && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setFilter("experience_id", undefined)}
          >
            Clear filter
          </Button>
        )}
      </div>

      {isLoading ? (
        <LoadingTable />
      ) : items.length === 0 ? (
        <EmptyState
          icon={AppWindow}
          title="No applications yet"
          description="Create your first application to add it to the catalog."
          action="New Application"
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
                <TableHead className="hidden sm:table-cell">Platform</TableHead>
                <TableHead className="hidden md:table-cell">Experience</TableHead>
                <TableHead className="hidden lg:table-cell">Description</TableHead>
                <TableHead className="hidden md:table-cell">Created At</TableHead>
                <TableHead className="w-[100px]" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((app) => (
                <TableRow key={app.id}>
                  <TableCell><CopyableId value={app.id} /></TableCell>
                  <TableCell className="font-medium">{app.name}</TableCell>
                  <TableCell className="hidden sm:table-cell text-sm">
                    {app.team_id ? teamName(app.team_id) : "—"}
                  </TableCell>
                  <TableCell className="hidden sm:table-cell text-sm text-muted-foreground">
                    {app.platform ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm">
                    {app.experience_id ? experienceName(app.experience_id) : "—"}
                  </TableCell>
                  <TableCell className="hidden lg:table-cell max-w-[200px] truncate text-muted-foreground text-sm">
                    {app.description ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                    {new Date(app.created_at).toLocaleDateString()}
                  </TableCell>
                  <TableCell>
                    <div className="flex items-center gap-1">
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Edit application"
                        onClick={() => setEditTarget(app)}
                      >
                        <Pencil className="h-4 w-4 text-muted-foreground" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Delete application"
                        onClick={() => setDeleteTarget(app)}
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
          Showing {Math.min(PAGE_SIZE, items.length)} of {total} applications
        </p>
      )}

      <ApplicationDialog
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
        title="Delete Application"
        description={`Are you sure you want to delete "${deleteTarget?.name}"? This cannot be undone.`}
        onConfirm={handleDelete}
        loading={deleteApplication.isPending}
      />
    </div>
  );
}
