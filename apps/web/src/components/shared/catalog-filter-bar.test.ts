import { describe, expect, it } from "vitest";
import { catalogFiltersFromUrl, withCatalogFilters } from "./catalog-filter-bar";

describe("catalog filter URL mapping", () => {
  it("reads the console's snake_case URL parameters as camelCase values", () => {
    const params = new URLSearchParams("experience_id=e1&page_id=p1&page=2");
    expect(catalogFiltersFromUrl(params)).toEqual({ experienceId: "e1", pageId: "p1" });
  });

  it("reads no filters from an empty query", () => {
    expect(catalogFiltersFromUrl(new URLSearchParams())).toEqual({});
  });

  it("writes values back to the same URL parameters", () => {
    const next = withCatalogFilters(new URLSearchParams(), {
      experienceId: "e1",
      applicationId: "a1",
      pageId: "p1",
      componentId: "c1",
    });
    expect(next.toString()).toBe("experience_id=e1&application_id=a1&page_id=p1&component_id=c1");
    expect(catalogFiltersFromUrl(next)).toEqual({
      experienceId: "e1",
      applicationId: "a1",
      pageId: "p1",
      componentId: "c1",
    });
  });

  it("deletes the parameter of a cleared value and keeps unrelated ones", () => {
    const prev = new URLSearchParams("experience_id=e1&application_id=a1&status=open&page=3");
    const next = withCatalogFilters(prev, { applicationId: "a1" });
    expect(next.get("experience_id")).toBeNull();
    expect(next.get("application_id")).toBe("a1");
    expect(next.get("status")).toBe("open");
    expect(next.get("page")).toBe("3");
    expect(prev.get("experience_id")).toBe("e1");
  });
});
