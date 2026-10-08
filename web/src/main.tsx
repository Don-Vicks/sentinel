import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter, Link, Route, Routes } from 'react-router';
import '@fontsource-variable/geist';
import '@fontsource-variable/geist-mono';
import './index.css';
import { ProgramsProvider } from './lib/programs';
import { AuthProvider } from './lib/auth';
import { SignInDialog } from './components/SignIn';
import { Layout } from './components/Layout';
import { Overview } from './pages/Overview';
import { Program } from './pages/Program';
import { Incident, Incidents } from './pages/Incident';
import { Transaction } from './pages/Transaction';
import { Alerts } from './pages/Alerts';
import { ProgramSummary } from './pages/Summary';
import { Empty } from './components/ui';

function NotFound() {
  return (
    <div className="panel">
      <Empty title="Page not found">
        <Link to="/" className="link">Back to overview</Link>
      </Empty>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AuthProvider>
    <ProgramsProvider>
      <SignInDialog />
      <BrowserRouter>
        <Routes>
          <Route element={<Layout />}>
            <Route index element={<Overview />} />
            <Route path="programs/:id" element={<Program />} />
            <Route path="programs/:id/summary" element={<ProgramSummary />} />
            <Route path="incidents" element={<Incidents />} />
            <Route path="incidents/:id" element={<Incident />} />
            <Route path="tx/:sig" element={<Transaction />} />
            <Route path="alerts" element={<Alerts />} />
            <Route path="*" element={<NotFound />} />
          </Route>
        </Routes>
      </BrowserRouter>
    </ProgramsProvider>
    </AuthProvider>
  </StrictMode>,
);
