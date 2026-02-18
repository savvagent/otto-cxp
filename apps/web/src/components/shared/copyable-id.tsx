import { Copy, Check } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

export function CopyableId({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);

  function handleCopy() {
    navigator.clipboard.writeText(value).then(() => {
      setCopied(true);
      toast.success("Copied!");
      setTimeout(() => setCopied(false), 2000);
    });
  }

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={handleCopy}
            className="inline-flex items-center gap-1 font-mono text-xs text-muted-foreground hover:text-foreground transition-colors cursor-pointer"
          >
            {value.slice(0, 8)}
            {copied ? (
              <Check className="h-3 w-3 text-green-600" />
            ) : (
              <Copy className="h-3 w-3" />
            )}
          </button>
        </TooltipTrigger>
        <TooltipContent>
          <p className="font-mono text-xs">{value}</p>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
