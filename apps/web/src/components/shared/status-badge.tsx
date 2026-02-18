import { Badge } from "@/components/ui/badge";

const statusColors: Record<string, string> = {
  open: "bg-blue-100 text-blue-800 hover:bg-blue-100",
  in_progress: "bg-yellow-100 text-yellow-800 hover:bg-yellow-100",
  waiting: "bg-orange-100 text-orange-800 hover:bg-orange-100",
  resolved: "bg-green-100 text-green-800 hover:bg-green-100",
  closed: "bg-gray-100 text-gray-800 hover:bg-gray-100",
};

export function StatusBadge({ status }: { status: string }) {
  return (
    <Badge variant="outline" className={statusColors[status] || ""}>
      {status.replace(/_/g, " ")}
    </Badge>
  );
}
