import { useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Layers, Pencil, Trash2 } from "lucide-react";

import {
  useExperiences,
  useCreateExperience,
  useUpdateExperience,
  useDeleteExperience,
} from "@/hooks/use-experiences";
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
import type { Experience } from "@/lib/types";

const PAGE_SIZE = 20;

const experienceSchema = z.object({
  name: z.string().min(1, "Name is required"),
  description: z.string().optional(),
});

type ExperienceForm = z.infer<typeof experienceSchema>;

function ExperienceDialog({
  open,
  onOpenChange,
  editing,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: Experience | null;
}) {
  const createExperience = useCreateExperience();
  const updateExperience = useUpdateExperience();
  const isEdit = !!editing;

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<ExperienceForm>({
    resolver: zodResolver(experienceSchema),
    defaultValues: {
      name: editing?.name ?? "",
      description: editing?.description ?? "",
    },
    values: editing
      ? { name: editing.name, description: editing.description ?? "" }
      : undefined,
  });

  function onSubmit(values: ExperienceForm) {
    const payload = {
      name: values.name,
      description: values.description || undefined,
    };

    if (isEdit && editing) {
      updateExperience.mutate(
        { id: editing.id, data: payload },
        {
          onSuccess: () => {
            toast.success("Experience updated");
            onOpenChange(false);
          },
          onError: (err) => toast.error(`Failed: ${err.message}`),
        }
      );
    } else {
      createExperience.mutate(payload, {
        onSuccess: () => {
          toast.success("Experience created");
          reset();
          onOpenChange(false);
        },
        onError: (err) => toast.error(`Failed: ${err.message}`),
      });
    }
  }

  const isPending = createExperience.isPending || updateExperience.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{isEdit ? "Edit Experience" : "New Experience"}</DialogTitle>
        </DialogHeader>
        <form onSubmit={handleSubmit(onSubmit)} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="exp-name">
              Name <span className="text-destructive">*</span>
            </Label>
            <Input id="exp-name" placeholder="Web App" {...register("name")} />
            {errors.name && (
              <p className="text-xs text-destructive">{errors.name.message}</p>
            )}
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="exp-desc">Description</Label>
            <Textarea
              id="exp-desc"
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

export function ExperiencesPage() {
  const [page] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<Experience | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Experience | null>(null);

  const { data, isLoading } = useExperiences({
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  });

  const deleteExperience = useDeleteExperience();
  const items = data?.data ?? [];
  const total = data?.total ?? 0;

  function handleDelete() {
    if (!deleteTarget) return;
    deleteExperience.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast.success("Experience deleted");
        setDeleteTarget(null);
      },
      onError: (err) => toast.error(`Failed: ${err.message}`),
    });
  }

  return (
    <div className="space-y-6">
      <PageHeader
        title="Experiences"
        action="New Experience"
        onAction={() => setCreateOpen(true)}
      />

      {isLoading ? (
        <LoadingTable />
      ) : items.length === 0 ? (
        <EmptyState
          icon={Layers}
          title="No experiences yet"
          description="Create your first experience to start building your catalog."
          action="New Experience"
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
                <TableHead className="hidden md:table-cell">Created At</TableHead>
                <TableHead className="w-[100px]" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((exp) => (
                <TableRow key={exp.id}>
                  <TableCell><CopyableId value={exp.id} /></TableCell>
                  <TableCell className="font-medium">{exp.name}</TableCell>
                  <TableCell className="hidden sm:table-cell max-w-[300px] truncate text-muted-foreground text-sm">
                    {exp.description ?? "—"}
                  </TableCell>
                  <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                    {new Date(exp.created_at).toLocaleDateString()}
                  </TableCell>
                  <TableCell>
                    <div className="flex items-center gap-1">
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Edit experience"
                        onClick={() => setEditTarget(exp)}
                      >
                        <Pencil className="h-4 w-4 text-muted-foreground" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Delete experience"
                        onClick={() => setDeleteTarget(exp)}
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
          Showing {Math.min(PAGE_SIZE, items.length)} of {total} experiences
        </p>
      )}

      <ExperienceDialog
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
        title="Delete Experience"
        description={`Are you sure you want to delete "${deleteTarget?.name}"? This cannot be undone.`}
        onConfirm={handleDelete}
        loading={deleteExperience.isPending}
      />
    </div>
  );
}
