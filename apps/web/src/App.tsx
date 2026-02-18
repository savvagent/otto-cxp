import { BrowserRouter, Routes, Route, Navigate } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "sonner";
import { RootLayout } from "@/components/layout/root-layout";
import { DashboardPage } from "@/pages/dashboard";
import { CustomersPage } from "@/pages/customers";
import { CustomerDetailPage } from "@/pages/customers/customer-detail";
import { TicketsPage } from "@/pages/tickets";
import { TicketDetailPage } from "@/pages/tickets/ticket-detail";
import { FeedbackPage } from "@/pages/feedback";
import { SurveysPage } from "@/pages/surveys";
import { SurveyDetailPage } from "@/pages/surveys/survey-detail";
import { ExperiencesPage } from "@/pages/catalog/experiences";
import { ApplicationsPage } from "@/pages/catalog/applications";
import { PagesPage } from "@/pages/catalog/pages-page";
import { ComponentsPage } from "@/pages/catalog/components-page";
import { CommandPalette } from "@/components/command-palette";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
    },
  },
});

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Routes>
          <Route element={<RootLayout />}>
            <Route index element={<Navigate to="/dashboard" replace />} />
            <Route path="dashboard" element={<DashboardPage />} />
            <Route path="customers" element={<CustomersPage />} />
            <Route path="customers/:id" element={<CustomerDetailPage />} />
            <Route path="tickets" element={<TicketsPage />} />
            <Route path="tickets/:id" element={<TicketDetailPage />} />
            <Route path="feedback" element={<FeedbackPage />} />
            <Route path="surveys" element={<SurveysPage />} />
            <Route path="surveys/:id" element={<SurveyDetailPage />} />
            <Route path="catalog/experiences" element={<ExperiencesPage />} />
            <Route path="catalog/applications" element={<ApplicationsPage />} />
            <Route path="catalog/pages" element={<PagesPage />} />
            <Route path="catalog/components" element={<ComponentsPage />} />
          </Route>
        </Routes>
        <CommandPalette />
      </BrowserRouter>
      <Toaster />
    </QueryClientProvider>
  );
}
