import { useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { UsersRound, Pencil, Trash2 } from "lucide-react";

import {
  useTeams,
  useCreateTeam,
  useUpdateTeam,
  useDeleteTeam,
} from "@/hooks/use-teams";
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import type { Team } from "@/lib/types";

const PAGE_SIZE = 20;

const teamSchema = z.object({
  name: z.string().min(1, "Name is required"),
  description: z.string().optional(),
  owner: z.string().optional(),
});

type TeamForm = z.infer<typeof teamSchema>;

function TeamDialog({
  open,
  onOpenChange,
  editing,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: Team | null;
}) {
  const createTeam = useCreateTeam();
  const updateTeam = useUpdateTeam();
  const isEdit = !!editing;

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<TeamForm>({
    resolver: zodResolver(teamSchema),
    defaultValues: {
      name: editing?.name ?? "",
      description: editing?.description ?? "",
      owner: editing?.owner ?? "",
    },
    values: editing
      ? { name: editing.name, description: editing.description ?? "", owner: editing.owner ?? "" }
      : undefined,
  });

  function onSubmit(values: TeamForm) {
    const payload = {
      name: values.name,
      description: values.description || undefined,
      owner: values.owner || undefined,
    };

    if (isEdit && editing) {
      updateTeam.mutate(
        { id: editing.id, data: payload },
        {
          onSuccess: () => {
            toast.success("Team updated");
            onOpenChange(false);
          },
          onError: (err) => toast.error(`Failed: ${err.message}`),
        }
      );
    } else {
      createTeam.mutate(payload, {
        onSuccess: () => {
          toast.success("Team created");
          reset();
          onOpenChange(false);
        },
        onError: (err) => toast.error(`Failed: ${err.message}`),
      });
    }
  }

  const isPending = createTeam.isPending || updateTeam.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{isEdit ? "Edit Team" : "New Team"}</DialogTitle>
        </DialogHeader>
        <form onSubmit={handleSubmit(onSubmit)} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="team-name">
              Name <span className="text-destructive">*</span>
            </Label>
            <Input id="team-name" placeholder="Platform Team" {...register("name")} />
            {errors.name && (
              <p className="text-xs text-destructive">{errors.name.message}</p>
            )}
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="team-desc">Description</Label>
            <Textarea
              id="team-desc"
              placeholder="Optional description..."
              rows={3}
              {...register("description")}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="team-owner">Owner</Label>
            <Input
              id="team-owner"
              placeholder="owner@example.com"
              {...register("owner")}
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

export function TeamsPage() {
  const [page] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<Team | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Team | null>(null);

  const { data, isLoading } = useTeams({
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  });

  const deleteTeam = useDeleteTeam();
  const items = data?.data ?? [];
  const total = data?.total ?? 0;

  function handleDelete() {
    if (!deleteTarget) return;
    deleteTeam.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast.success("Team deleted");
        setDeleteTarget(null);
      },
      onError: (err) => toast.error(`Failed: ${err.message}`),
    });
  }

  return (
    <div className="space-y-6">
      <PageHeader
        title="Teams"
        action="New Team"
        onAction={() => setCreateOpen(true)}
      />

      {isLoading ? (
        <LoadingTable />
      ) : items.length === 0 ? (
        <EmptyState
          icon={UsersRound}
          title="No teams yet"
          description="Create your first team to start organizing ownership."
          action="New Team"
          onAction={() => setCreateOpen(true)}
        />
      ) : (
        <div className="rounded-md border overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-[100px]">ID</TableHead>
                <TableHead>Name</TableHead>
                <TableHead className="hidden sm:table-cell">Description</TableHead>
                <TableHead className="hidden md:table-cell">Owner</TableHead>
                <TableHead className="hidden md:table-cell">Created At</TableHead>
                <TableHead className="w-[100px]" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((team) => (
                <TableRow key={team.id}>
                  <TableCell><CopyableId value={team.id} /></TableCell>
                  <TableCell className="font-medium">{team.name}</TableCell>
                  <TableCell className="hidden sm:table-cell max-w-[300px] truncate text-muted-foreground text-sm">
                    {team.description ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                    {team.owner ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                    {new Date(team.created_at).toLocaleDateString()}
                  </TableCell>
                  <TableCell>
                    <div className="flex items-center gap-1">
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Edit team"
                        onClick={() => setEditTarget(team)}
                      >
                        <Pencil className="h-4 w-4 text-muted-foreground" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Delete team"
                        onClick={() => setDeleteTarget(team)}
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
          Showing {Math.min(PAGE_SIZE, items.length)} of {total} teams
        </p>
      )}

      <TeamDialog
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
        title="Delete Team"
        description={`Are you sure you want to delete "${deleteTarget?.name}"? This cannot be undone.`}
        onConfirm={handleDelete}
        loading={deleteTeam.isPending}
      />
    </div>
  );
}
