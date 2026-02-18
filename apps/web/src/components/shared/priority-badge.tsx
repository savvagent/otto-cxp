import { Badge } from "@/components/ui/badge";

const priorityColors: Record<string, string> = {
  urgent: "bg-red-100 text-red-800 hover:bg-red-100",
  high: "bg-orange-100 text-orange-800 hover:bg-orange-100",
  medium: "bg-yellow-100 text-yellow-800 hover:bg-yellow-100",
  low: "bg-green-100 text-green-800 hover:bg-green-100",
};

export function PriorityBadge({ priority }: { priority: string }) {
  return (
    <Badge variant="outline" className={priorityColors[priority] || ""}>
      {priority}
    </Badge>
  );
}
