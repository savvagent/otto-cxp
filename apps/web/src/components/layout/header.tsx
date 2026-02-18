import { useLocation } from "react-router";
import { Menu, Search } from "lucide-react";
import { Button } from "@/components/ui/button";

const routeNames: Record<string, string> = {
  "/dashboard": "Dashboard",
  "/customers": "Customers",
  "/tickets": "Tickets",
  "/feedback": "Feedback",
  "/surveys": "Surveys",
  "/catalog/experiences": "Experiences",
  "/catalog/applications": "Applications",
  "/catalog/pages": "Pages",
  "/catalog/components": "Components",
};

export function Header({
  onMenuClick,
  onSearchClick,
}: {
  onMenuClick: () => void;
  onSearchClick: () => void;
}) {
  const location = useLocation();
  const basePath = "/" + location.pathname.split("/").filter(Boolean).slice(0, 2).join("/");
  const title = routeNames[basePath] || routeNames["/" + location.pathname.split("/")[1]] || "CXP";

  return (
    <header className="flex h-14 items-center gap-4 border-b bg-card px-4 lg:px-6">
      <Button
        variant="ghost"
        size="icon"
        className="md:hidden"
        onClick={onMenuClick}
      >
        <Menu className="h-5 w-5" />
      </Button>

      <h1 className="text-lg font-semibold">{title}</h1>

      <div className="ml-auto flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          className="hidden gap-2 text-muted-foreground sm:flex"
          onClick={onSearchClick}
        >
          <Search className="h-4 w-4" />
          <span>Search...</span>
          <kbd className="pointer-events-none rounded border bg-muted px-1.5 py-0.5 text-xs">
            ⌘K
          </kbd>
        </Button>
      </div>
    </header>
  );
}
