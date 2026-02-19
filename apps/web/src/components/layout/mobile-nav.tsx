import { NavLink } from "react-router";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  LayoutDashboard,
  Users,
  Ticket,
  MessageSquare,
  ClipboardList,
  Layers,
  AppWindow,
  FileText,
  Puzzle,
  UsersRound,
  Zap,
} from "lucide-react";
import { cn } from "@/lib/utils";

const allItems = [
  { to: "/dashboard", label: "Dashboard", icon: LayoutDashboard },
  { to: "/customers", label: "Customers", icon: Users },
  { to: "/tickets", label: "Tickets", icon: Ticket },
  { to: "/feedback", label: "Feedback", icon: MessageSquare },
  { to: "/surveys", label: "Surveys", icon: ClipboardList },
  { to: "/catalog/experiences", label: "Experiences", icon: Layers },
  { to: "/catalog/applications", label: "Applications", icon: AppWindow },
  { to: "/catalog/pages", label: "Pages", icon: FileText },
  { to: "/catalog/components", label: "Components", icon: Puzzle },
  { to: "/catalog/teams", label: "Teams", icon: UsersRound },
];

export function MobileNav({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  return (
    <Sheet open={open} onOpenChange={onClose}>
      <SheetContent side="left" className="w-72 p-0">
        <SheetHeader className="border-b px-4 py-3">
          <SheetTitle className="flex items-center gap-2">
            <Zap className="h-5 w-5 text-primary" />
            CXP
          </SheetTitle>
        </SheetHeader>
        <nav className="space-y-1 p-3">
          {allItems.map(({ to, label, icon: Icon }) => (
            <NavLink
              key={to}
              to={to}
              onClick={onClose}
              className={({ isActive }) =>
                cn(
                  "flex items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors",
                  isActive
                    ? "bg-primary text-primary-foreground"
                    : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                )
              }
            >
              <Icon className="h-4 w-4" />
              <span>{label}</span>
            </NavLink>
          ))}
        </nav>
      </SheetContent>
    </Sheet>
  );
}
