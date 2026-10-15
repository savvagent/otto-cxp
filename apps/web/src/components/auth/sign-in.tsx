import { useLocation } from "react-router";
import { LogIn, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import { loginErrorMessage, loginUrl, returnPath } from "@/lib/session";

/** Shown instead of the console while the browser is not signed in. */
export function SignIn({ loginEnabled }: { loginEnabled: boolean }) {
  // Re-render on client-side navigation; the path itself comes from the
  // browser, whose path (unlike the router's) includes `/console/`.
  const location = useLocation();
  const error = loginErrorMessage(location.search);
  const next = returnPath(window.location);

  return (
    <main className="flex min-h-screen items-center justify-center bg-background p-4">
      <div className="w-full max-w-sm space-y-6 rounded-lg border bg-card p-8 text-center shadow-sm">
        <div className="flex items-center justify-center gap-2 text-lg font-semibold">
          <Zap className="h-5 w-5 text-primary" aria-hidden="true" />
          <h1>Sign in to CXP</h1>
        </div>
        {loginEnabled ? (
          <>
            <p className="text-sm text-muted-foreground">
              Sign in with your otto account to continue.
            </p>
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {error}
              </p>
            )}
            <Button asChild className="w-full">
              <a href={loginUrl(next)}>
                <LogIn className="h-4 w-4" aria-hidden="true" />
                Sign in with otto
              </a>
            </Button>
          </>
        ) : (
          <p className="text-sm text-muted-foreground">
            Console sign-in is not configured on this server. API and MCP clients
            authenticate with their own otto tokens.
          </p>
        )}
      </div>
    </main>
  );
}
