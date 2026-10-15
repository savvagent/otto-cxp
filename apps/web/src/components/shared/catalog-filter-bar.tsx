import { useExperiences } from "@/hooks/use-experiences";
import { useApplications } from "@/hooks/use-applications";
import { usePages } from "@/hooks/use-pages";
import { useComponents } from "@/hooks/use-components";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { FilterX } from "lucide-react";

export interface CatalogFilterValues {
  experienceId?: string;
  applicationId?: string;
  pageId?: string;
  componentId?: string;
}

/** The console's own URL parameter for each catalog filter (router state, not the API wire). */
const URL_PARAM: Record<keyof CatalogFilterValues, string> = {
  experienceId: "experience_id",
  applicationId: "application_id",
  pageId: "page_id",
  componentId: "component_id",
};

export function catalogFiltersFromUrl(params: URLSearchParams): CatalogFilterValues {
  const out: CatalogFilterValues = {};
  for (const [key, name] of Object.entries(URL_PARAM) as [keyof CatalogFilterValues, string][]) {
    const v = params.get(name);
    if (v) out[key] = v;
  }
  return out;
}

export function withCatalogFilters(prev: URLSearchParams, values: CatalogFilterValues): URLSearchParams {
  const next = new URLSearchParams(prev);
  for (const [key, name] of Object.entries(URL_PARAM) as [keyof CatalogFilterValues, string][]) {
    const v = values[key];
    if (v) next.set(name, v);
    else next.delete(name);
  }
  return next;
}

interface CatalogFilterBarProps {
  value: CatalogFilterValues;
  onChange: (value: CatalogFilterValues) => void;
}

export function CatalogFilterBar({ value, onChange }: CatalogFilterBarProps) {
  const { data: experiencesData } = useExperiences({ limit: 200 });
  const { data: applicationsData } = useApplications({ limit: 200 });
  const { data: pagesData } = usePages({ limit: 200 });
  const { data: componentsData } = useComponents({ limit: 200 });

  const experiences = experiencesData?.data ?? [];
  const applications = applicationsData?.data ?? [];
  const pages = pagesData?.data ?? [];
  const components = componentsData?.data ?? [];

  const hasFilters = !!(
    value.experienceId ||
    value.applicationId ||
    value.pageId ||
    value.componentId
  );

  function setFilter(key: keyof CatalogFilterValues, val: string | undefined) {
    onChange({ ...value, [key]: val });
  }

  function clearAll() {
    onChange({});
  }

  return (
    <div className="flex flex-wrap items-center gap-3">
      <Select
        value={value.experienceId ?? "_all"}
        onValueChange={(v) =>
          setFilter("experienceId", v === "_all" ? undefined : v)
        }
      >
        <SelectTrigger className="w-[180px]" aria-label="Filter by experience">
          <SelectValue placeholder="All experiences" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="_all">All experiences</SelectItem>
          {experiences.map((e) => (
            <SelectItem key={e.id} value={e.id}>
              {e.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <Select
        value={value.applicationId ?? "_all"}
        onValueChange={(v) =>
          setFilter("applicationId", v === "_all" ? undefined : v)
        }
      >
        <SelectTrigger
          className="w-[180px]"
          aria-label="Filter by application"
        >
          <SelectValue placeholder="All applications" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="_all">All applications</SelectItem>
          {applications.map((a) => (
            <SelectItem key={a.id} value={a.id}>
              {a.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <Select
        value={value.pageId ?? "_all"}
        onValueChange={(v) =>
          setFilter("pageId", v === "_all" ? undefined : v)
        }
      >
        <SelectTrigger className="w-[180px]" aria-label="Filter by page">
          <SelectValue placeholder="All pages" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="_all">All pages</SelectItem>
          {pages.map((p) => (
            <SelectItem key={p.id} value={p.id}>
              {p.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <Select
        value={value.componentId ?? "_all"}
        onValueChange={(v) =>
          setFilter("componentId", v === "_all" ? undefined : v)
        }
      >
        <SelectTrigger className="w-[180px]" aria-label="Filter by component">
          <SelectValue placeholder="All components" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="_all">All components</SelectItem>
          {components.map((c) => (
            <SelectItem key={c.id} value={c.id}>
              {c.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      {hasFilters && (
        <Button
          variant="ghost"
          size="sm"
          onClick={clearAll}
          className="gap-1.5"
        >
          <FilterX className="h-4 w-4" />
          Clear catalog filters
        </Button>
      )}
    </div>
  );
}
