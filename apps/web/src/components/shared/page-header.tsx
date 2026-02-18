import { Button } from "@/components/ui/button";
import { Plus } from "lucide-react";

export function PageHeader({
  title,
  description,
  action,
  onAction,
}: {
  title: string;
  description?: string;
  action?: string;
  onAction?: () => void;
}) {
  return (
    <div className="flex items-center justify-between">
      <div>
        <h2 className="text-2xl font-bold tracking-tight">{title}</h2>
        {description && (
          <p className="text-muted-foreground">{description}</p>
        )}
      </div>
      {action && onAction && (
        <Button onClick={onAction}>
          <Plus className="mr-2 h-4 w-4" />
          {action}
        </Button>
      )}
    </div>
  );
}
