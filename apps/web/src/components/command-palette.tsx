import { useEffect, useState, useCallback } from "react";
import { useNavigate } from "react-router";
import { Command } from "cmdk";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { customersApi, ticketsApi } from "@/lib/api";
import type { Customer, Ticket } from "@/lib/types";
import {
  Users,
  Ticket as TicketIcon,
  LayoutDashboard,
  MessageSquare,
  ClipboardList,
  Layers,
  Search,
} from "lucide-react";

export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [customers, setCustomers] = useState<Customer[]>([]);
  const [tickets, setTickets] = useState<Ticket[]>([]);
  const [loading, setLoading] = useState(false);
  const navigate = useNavigate();

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        setOpen((prev) => !prev);
      }
    };
    const handleCustomEvent = () => setOpen(true);

    document.addEventListener("keydown", handleKeyDown);
    window.addEventListener("open-command-palette", handleCustomEvent);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("open-command-palette", handleCustomEvent);
    };
  }, []);

  const search = useCallback(async (q: string) => {
    if (q.length < 2) {
      setCustomers([]);
      setTickets([]);
      return;
    }
    setLoading(true);
    try {
      const [customerResults, ticketResults] = await Promise.allSettled([
        customersApi.search(q),
        ticketsApi.list({ limit: 10 }),
      ]);
      if (customerResults.status === "fulfilled") {
        setCustomers(customerResults.value.slice(0, 5));
      }
      if (ticketResults.status === "fulfilled") {
        const filtered = ticketResults.value.data.filter(
          (t) =>
            t.subject.toLowerCase().includes(q.toLowerCase()) ||
            t.id.startsWith(q),
        );
        setTickets(filtered.slice(0, 5));
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    const timer = setTimeout(() => search(query), 200);
    return () => clearTimeout(timer);
  }, [query, search]);

  const go = (path: string) => {
    setOpen(false);
    setQuery("");
    navigate(path);
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="overflow-hidden p-0 shadow-lg max-w-lg">
        <Command className="[&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:text-muted-foreground [&_[cmdk-group]]:px-2 [&_[cmdk-item]]:px-2 [&_[cmdk-item]]:py-2 [&_[cmdk-input]]:h-12">
          <div className="flex items-center border-b px-3">
            <Search className="mr-2 h-4 w-4 shrink-0 opacity-50" />
            <Command.Input
              placeholder="Search customers, tickets, or navigate..."
              value={query}
              onValueChange={setQuery}
              className="flex h-11 w-full rounded-md bg-transparent py-3 text-sm outline-none placeholder:text-muted-foreground"
            />
          </div>
          <Command.List className="max-h-[300px] overflow-y-auto p-2">
            <Command.Empty className="py-6 text-center text-sm text-muted-foreground">
              {loading ? "Searching..." : "No results found."}
            </Command.Empty>

            <Command.Group heading="Navigation">
              <Command.Item onSelect={() => go("/dashboard")} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm cursor-pointer hover:bg-accent">
                <LayoutDashboard className="h-4 w-4" /> Dashboard
              </Command.Item>
              <Command.Item onSelect={() => go("/customers")} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm cursor-pointer hover:bg-accent">
                <Users className="h-4 w-4" /> Customers
              </Command.Item>
              <Command.Item onSelect={() => go("/tickets")} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm cursor-pointer hover:bg-accent">
                <TicketIcon className="h-4 w-4" /> Tickets
              </Command.Item>
              <Command.Item onSelect={() => go("/feedback")} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm cursor-pointer hover:bg-accent">
                <MessageSquare className="h-4 w-4" /> Feedback
              </Command.Item>
              <Command.Item onSelect={() => go("/surveys")} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm cursor-pointer hover:bg-accent">
                <ClipboardList className="h-4 w-4" /> Surveys
              </Command.Item>
              <Command.Item onSelect={() => go("/catalog/experiences")} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm cursor-pointer hover:bg-accent">
                <Layers className="h-4 w-4" /> Catalog
              </Command.Item>
            </Command.Group>

            {customers.length > 0 && (
              <Command.Group heading="Customers">
                {customers.map((c) => (
                  <Command.Item
                    key={c.id}
                    value={`customer-${c.id}`}
                    onSelect={() => go(`/customers/${c.id}`)}
                    className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm cursor-pointer hover:bg-accent"
                  >
                    <Users className="h-4 w-4" />
                    <span>{c.name || c.email}</span>
                    {c.company && (
                      <span className="ml-auto text-xs text-muted-foreground">
                        {c.company}
                      </span>
                    )}
                  </Command.Item>
                ))}
              </Command.Group>
            )}

            {tickets.length > 0 && (
              <Command.Group heading="Tickets">
                {tickets.map((t) => (
                  <Command.Item
                    key={t.id}
                    value={`ticket-${t.id}`}
                    onSelect={() => go(`/tickets/${t.id}`)}
                    className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm cursor-pointer hover:bg-accent"
                  >
                    <TicketIcon className="h-4 w-4" />
                    <span className="truncate">{t.subject}</span>
                    <span className="ml-auto text-xs text-muted-foreground">
                      {t.status}
                    </span>
                  </Command.Item>
                ))}
              </Command.Group>
            )}
          </Command.List>
        </Command>
      </DialogContent>
    </Dialog>
  );
}
