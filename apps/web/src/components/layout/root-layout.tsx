import { useState, useCallback } from "react";
import { Outlet } from "react-router";
import { Sidebar } from "./sidebar";
import { Header } from "./header";
import { MobileNav } from "./mobile-nav";
import { SignIn } from "@/components/auth/sign-in";
import { Skeleton } from "@/components/ui/skeleton";
import { useSession } from "@/hooks/use-session";

/** The console, once the browser is signed in; the sign-in screen until then. */
export function RootLayout() {
  const session = useSession();
  if (session.isPending) {
    return (
      <div className="flex min-h-screen items-center justify-center p-4">
        <Skeleton className="h-32 w-full max-w-sm rounded-lg" />
      </div>
    );
  }
  if (session.isError) {
    return (
      <div className="flex min-h-screen items-center justify-center p-4 text-sm text-muted-foreground">
        Could not reach otto-cxp. Reload to try again.
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
