import { Button } from "@/components/ui/button";
import { InboxIcon } from "lucide-react";

export function EmptyState({
  icon: Icon = InboxIcon,
  title,
  description,
  action,
  onAction,
}: {
  icon?: React.ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  action?: string;
  onAction?: () => void;
}) {
  return (
    <div className="flex flex-col items-center justify-center py-12 text-center">
      <Icon className="h-12 w-12 text-muted-foreground/50" />
      <h3 className="mt-4 text-lg font-semibold">{title}</h3>
      {description && (
        <p className="mt-1 text-sm text-muted-foreground">{description}</p>
      )}
      {action && onAction && (
        <Button onClick={onAction} className="mt-4">
          {action}
        </Button>
      )}
    </div>
  );
}
