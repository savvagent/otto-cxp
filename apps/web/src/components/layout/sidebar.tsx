import { NavLink } from "react-router";
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
  ChevronDown,
  Zap,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useState } from "react";

const navItems = [
  { to: "/dashboard", label: "Dashboard", icon: LayoutDashboard },
  { to: "/customers", label: "Customers", icon: Users },
  { to: "/tickets", label: "Tickets", icon: Ticket },
  { to: "/feedback", label: "Feedback", icon: MessageSquare },
  { to: "/surveys", label: "Surveys", icon: ClipboardList },
];

const catalogItems = [
  { to: "/catalog/experiences", label: "Experiences", icon: Layers },
  { to: "/catalog/applications", label: "Applications", icon: AppWindow },
  { to: "/catalog/pages", label: "Pages", icon: FileText },
  { to: "/catalog/components", label: "Components", icon: Puzzle },
];

function NavItem({
  to,
  label,
  icon: Icon,
  collapsed,
}: {
  to: string;
  label: string;
  icon: React.ComponentType<{ className?: string }>;
  collapsed: boolean;
}) {
  return (
    <NavLink
      to={to}
      className={({ isActive }) =>
        cn(
          "flex items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors",
          isActive
            ? "bg-primary text-primary-foreground"
            : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
          collapsed && "justify-center px-2",
        )
      }
    >
      <Icon className="h-4 w-4 shrink-0" />
      {!collapsed && <span>{label}</span>}
    </NavLink>
  );
}

export function Sidebar({
  collapsed,
  onToggle,
}: {
  collapsed: boolean;
  onToggle: () => void;
}) {
  const [catalogOpen, setCatalogOpen] = useState(true);

  return (
    <aside
      className={cn(
        "hidden border-r bg-card transition-all duration-200 md:flex md:flex-col",
        collapsed ? "w-16" : "w-64",
      )}
    >
      <div className="flex h-14 items-center border-b px-4">
        <button onClick={onToggle} className="flex items-center gap-2">
          <Zap className="h-5 w-5 text-primary" />
          {!collapsed && (
            <span className="font-semibold text-lg">CXP</span>
          )}
        </button>
      </div>

      <nav className="flex-1 space-y-1 overflow-y-auto p-3">
        {navItems.map((item) => (
          <NavItem key={item.to} {...item} collapsed={collapsed} />
        ))}

        {!collapsed && (
          <div className="pt-4">
            <button
              onClick={() => setCatalogOpen(!catalogOpen)}
              className="flex w-full items-center gap-2 px-3 py-2 text-xs font-medium uppercase text-muted-foreground"
            >
              <span>Catalog</span>
              <ChevronDown
                className={cn(
                  "h-3 w-3 transition-transform",
                  catalogOpen && "rotate-180",
                )}
              />
            </button>
            {catalogOpen &&
              catalogItems.map((item) => (
                <NavItem key={item.to} {...item} collapsed={collapsed} />
              ))}
          </div>
        )}

        {collapsed &&
          catalogItems.map((item) => (
            <NavItem key={item.to} {...item} collapsed={collapsed} />
          ))}
      </nav>
    </aside>
  );
}
