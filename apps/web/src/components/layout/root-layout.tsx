import { useState, useCallback } from "react";
import { Outlet } from "react-router";
import { Sidebar } from "./sidebar";
import { Header } from "./header";
import { MobileNav } from "./mobile-nav";
import { SignIn } from "@/components/auth/sign-in";
import { RotateCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useSession } from "@/hooks/use-session";

/** The console, once the browser is signed in; the sign-in screen until then. */
export function RootLayout() {
  const session = useSession();
  if (session.isPending) {
    return (
      <div
        className="flex min-h-screen items-center justify-center p-4"
        aria-busy="true"
        aria-label="Checking your sign-in"
      >
        <Skeleton className="h-32 w-full max-w-sm rounded-lg" />
      </div>
    );
  }
  if (session.isError) {
    return (
      <div
        className="flex min-h-screen flex-col items-center justify-center gap-3 p-4 text-sm text-muted-foreground"
        aria-busy={session.isFetching}
      >
        <p role="alert">Could not reach Savvagent CXP to check your sign-in.</p>
        <Button
          variant="outline"
          size="sm"
          disabled={session.isFetching}
          onClick={() => void session.refetch()}
        >
          <RotateCw className="h-4 w-4" />
          Try again
        </Button>
      </div>
    );
  }
  if (!session.data.signedIn) {
    return <SignIn loginEnabled={session.data.loginEnabled} />;
  }
  return <Console />;
}

function Console() {
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [mobileNavOpen, setMobileNavOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);

  const handleSearchClick = useCallback(() => {
    setSearchOpen(true);
    // Dispatch custom event for command palette
    window.dispatchEvent(new CustomEvent("open-command-palette"));
  }, []);

  return (
    <div className="flex h-screen overflow-hidden">
      <Sidebar
        collapsed={sidebarCollapsed}
        onToggle={() => setSidebarCollapsed(!sidebarCollapsed)}
      />
      <MobileNav
        open={mobileNavOpen}
        onClose={() => setMobileNavOpen(false)}
      />
      <div className="flex flex-1 flex-col overflow-hidden">
        <Header
          onMenuClick={() => setMobileNavOpen(true)}
          onSearchClick={handleSearchClick}
        />
        <main className="flex-1 overflow-y-auto p-4 lg:p-6">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
