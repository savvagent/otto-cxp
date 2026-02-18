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
  experience_id?: string;
  application_id?: string;
  page_id?: string;
  component_id?: string;
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
    value.experience_id ||
    value.application_id ||
    value.page_id ||
    value.component_id
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
        value={value.experience_id ?? "_all"}
        onValueChange={(v) =>
          setFilter("experience_id", v === "_all" ? undefined : v)
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
        value={value.application_id ?? "_all"}
        onValueChange={(v) =>
          setFilter("application_id", v === "_all" ? undefined : v)
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
        value={value.page_id ?? "_all"}
        onValueChange={(v) =>
          setFilter("page_id", v === "_all" ? undefined : v)
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
        value={value.component_id ?? "_all"}
        onValueChange={(v) =>
          setFilter("component_id", v === "_all" ? undefined : v)
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
