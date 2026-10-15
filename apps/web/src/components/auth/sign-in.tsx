import { useLocation } from "react-router";
import { LogIn, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import { loginErrorMessage, loginUrl } from "@/lib/session";

/** Shown instead of the console while the browser is not signed in. */
export function SignIn({ loginEnabled }: { loginEnabled: boolean }) {
  const location = useLocation();
  const error = loginErrorMessage(location.search);
  // Back to where the person was going, minus the failed attempt's marker.
  const next = error ? "/" : location.pathname + location.search;

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <div className="w-full max-w-sm space-y-6 rounded-lg border bg-card p-8 text-center shadow-sm">
        <div className="flex items-center justify-center gap-2 text-lg font-semibold">
          <Zap className="h-5 w-5 text-primary" />
          <span>CXP</span>
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
                <LogIn className="h-4 w-4" />
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
    </div>
  );
}
