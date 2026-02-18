import { useDashboardMetrics } from "@/hooks/use-dashboard";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Users, Ticket, MessageSquare, ClipboardList, Layers, Activity } from "lucide-react";

interface MetricCardProps {
  title: string;
  value: number | undefined;
  icon: React.ComponentType<{ className?: string }>;
  loading: boolean;
}

function MetricCard({ title, value, icon: Icon, loading }: MetricCardProps) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground">
          {title}
        </CardTitle>
        <Icon className="h-4 w-4 text-muted-foreground" />
      </CardHeader>
      <CardContent>
        {loading ? (
          <Skeleton className="h-8 w-24" />
        ) : (
          <div className="text-3xl font-bold">
            {value?.toLocaleString() ?? "—"}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

export function DashboardPage() {
  const { data: metrics, isLoading } = useDashboardMetrics();

  const cards = [
    {
      title: "Total Customers",
      value: metrics?.total_customers,
      icon: Users,
    },
    {
      title: "Open Tickets",
      value: metrics?.open_tickets,
      icon: Ticket,
    },
    {
      title: "Total Feedback",
      value: metrics?.total_feedback,
      icon: MessageSquare,
    },
    {
      title: "Total Surveys",
      value: metrics?.total_surveys,
      icon: ClipboardList,
    },
    {
      title: "Total Experiences",
      value: metrics?.total_experiences,
      icon: Layers,
    },
  ];

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight">Dashboard</h2>
        <p className="text-muted-foreground">
          Overview of your customer experience platform
        </p>
      </div>

      <div className="grid gap-4 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-5">
        {cards.map((card) => (
          <MetricCard
            key={card.title}
            title={card.title}
            value={card.value}
            icon={card.icon}
            loading={isLoading}
          />
        ))}
      </div>

      <Card>
        <CardHeader className="flex flex-row items-center gap-2">
          <Activity className="h-5 w-5 text-muted-foreground" />
          <CardTitle className="text-base">Recent Activity</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-sm text-muted-foreground">
            Dashboard charts coming soon
          </p>
        </CardContent>
      </Card>
    </div>
  );
}
