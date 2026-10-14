-- Demo data for ONE org: a small catalog, customers, tickets, feedback, surveys,
-- chat, and interactions.
--
--   psql "$DATABASE_URL" -v org=<org-uuid> -f scripts/seed-demo.sql
--
-- LOCAL POSTGRES ONLY (compose.yaml, port 15435). Running this against otto-db
-- is a production write and is human-only (Rob), with the real org's uuid.
-- It does not create the org: an org is a platform record, and this database
-- holds `org_id` as an opaque uuid.
--
-- Every row carries org_id = :'org'. Row ids are derived from the org uuid, so
-- the script is idempotent per org (re-running is a no-op) and can be run once
-- for each of several orgs in the same database.
-- Requires the schema from crates/cxp-core/migrations (apply by starting
-- cxp-server once, or running the migration with psql).

-- (customers are UNIQUE (org_id, email): if the org already has customers with
-- these demo emails under other ids, the insert fails and nothing is written.)

\if :{?org}
\else
  \echo 'usage: psql "$DATABASE_URL" -v org=<org-uuid> -f scripts/seed-demo.sql'
  \quit
\endif

\set ON_ERROR_STOP on

BEGIN;

-- Pin the org for row-level security, so the script also works as a non-superuser
-- table owner (FORCE ROW LEVEL SECURITY applies to owners).
SELECT set_config('app.org_id', :'org', true);

-- ============================================================
-- Catalog hierarchy: Experience > Application > Page > Component
-- ============================================================

INSERT INTO experiences (org_id, id, name, description) VALUES
  (:'org'::uuid, md5(:'org' || 'a0000000-0000-0000-0000-000000000001')::uuid, 'E-Commerce', 'Online shopping experience'),
  (:'org'::uuid, md5(:'org' || 'a0000000-0000-0000-0000-000000000002')::uuid, 'Mobile Banking', 'Mobile banking experience')
ON CONFLICT (id) DO NOTHING;

INSERT INTO applications (org_id, id, experience_id, name, platform, description) VALUES
  (:'org'::uuid, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || 'a0000000-0000-0000-0000-000000000001')::uuid, 'Web Store', 'web', 'Main e-commerce website'),
  (:'org'::uuid, md5(:'org' || 'b0000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || 'a0000000-0000-0000-0000-000000000001')::uuid, 'iOS Shopping App', 'ios', 'Native iOS shopping application'),
  (:'org'::uuid, md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || 'a0000000-0000-0000-0000-000000000002')::uuid, 'Banking App', 'ios', 'Mobile banking iOS app')
ON CONFLICT (id) DO NOTHING;

INSERT INTO pages (org_id, id, application_id, name, path, description) VALUES
  (:'org'::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, 'Product Listing', '/products', 'Browse all products'),
  (:'org'::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, 'Checkout', '/checkout', 'Cart and payment'),
  (:'org'::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, 'Account Settings', '/settings', 'User profile and preferences'),
  (:'org'::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000004')::uuid, md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid, 'Dashboard', '/dashboard', 'Banking dashboard')
ON CONFLICT (id) DO NOTHING;

INSERT INTO components (org_id, id, page_id, name, component_type, description) VALUES
  (:'org'::uuid, md5(:'org' || 'd0000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000001')::uuid, 'Search Bar', 'input', 'Product search'),
  (:'org'::uuid, md5(:'org' || 'd0000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000002')::uuid, 'Payment Form', 'form', 'Credit card entry'),
  (:'org'::uuid, md5(:'org' || 'd0000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000003')::uuid, 'Profile Editor', 'form', 'Edit profile fields'),
  (:'org'::uuid, md5(:'org' || 'd0000000-0000-0000-0000-000000000004')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000004')::uuid, 'Balance Widget', 'widget', 'Account balance display')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Customers
-- ============================================================

INSERT INTO customers (org_id, id, email, name, phone, company, tags, application_id) VALUES
  (:'org'::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, 'alice.chen@example.com', 'Alice Chen', '+1-555-0101', 'Acme Corp', ARRAY['vip','enterprise'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000002')::uuid, 'bob.martinez@example.com', 'Bob Martinez', '+1-555-0102', 'Globex Inc', ARRAY['trial'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000003')::uuid, 'carol.johnson@example.com', 'Carol Johnson', '+1-555-0103', NULL, ARRAY['free-tier'], md5(:'org' || 'b0000000-0000-0000-0000-000000000002')::uuid),
  (:'org'::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000004')::uuid, 'david.kim@example.com', 'David Kim', NULL, 'StartupXYZ', ARRAY['enterprise','beta'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000005')::uuid, 'eva.williams@example.com', 'Eva Williams', '+1-555-0105', 'Initech', ARRAY['vip'], md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid),
  (:'org'::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000006')::uuid, 'frank.nguyen@example.com', 'Frank Nguyen', '+1-555-0106', NULL, ARRAY['free-tier'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000007')::uuid, 'grace.patel@example.com', 'Grace Patel', '+1-555-0107', 'TechNova', ARRAY['enterprise'], md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid),
  (:'org'::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000008')::uuid, 'henry.lee@example.com', 'Henry Lee', NULL, 'ByteForge', ARRAY['trial','beta'], md5(:'org' || 'b0000000-0000-0000-0000-000000000002')::uuid)
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Tickets (various statuses, priorities, channels)
-- ============================================================

INSERT INTO tickets (org_id, id, customer_id, subject, description, status, priority, channel, assigned_to, tags, application_id) VALUES
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, 'Unable to complete checkout', 'Getting a 500 error on the payment step after entering credit card details.', 'open', 'high', 'web', 'agent-sarah', ARRAY['checkout','payment'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000002')::uuid, 'Wrong item shipped', 'Ordered a blue shirt (SKU-1234) but received a red one.', 'in_progress', 'medium', 'email', 'agent-mike', ARRAY['shipping','fulfillment'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000003')::uuid, 'App crashes on login', 'iOS app crashes immediately after entering credentials. iPhone 15, iOS 18.', 'open', 'urgent', 'chat', NULL, ARRAY['crash','ios','login'], md5(:'org' || 'b0000000-0000-0000-0000-000000000002')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000004')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000004')::uuid, 'Request bulk discount pricing', 'We want to order 500+ units. Looking for enterprise pricing.', 'waiting', 'low', 'email', 'agent-sarah', ARRAY['sales','enterprise'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000005')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000005')::uuid, 'Cannot reset password', 'Reset email never arrives. Checked spam folder.', 'in_progress', 'high', 'phone', 'agent-mike', ARRAY['auth','email'], md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000006')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, 'Search returns irrelevant results', 'Searching for "winter jacket" shows summer clothing.', 'open', 'medium', 'web', NULL, ARRAY['search','ux'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000007')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000006')::uuid, 'Promo code not working', 'SAVE20 code shows "invalid" even though the promotion is active.', 'resolved', 'medium', 'chat', 'agent-sarah', ARRAY['promo','checkout'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000008')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000007')::uuid, 'Transfer failed but money deducted', 'Attempted transfer of $500 failed but balance was still deducted.', 'open', 'urgent', 'phone', 'agent-mike', ARRAY['transfer','urgent','finance'], md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000009')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000008')::uuid, 'Feature request: dark mode', 'Would love a dark mode option in the app.', 'closed', 'low', 'web', NULL, ARRAY['feature-request','ui'], md5(:'org' || 'b0000000-0000-0000-0000-000000000002')::uuid),
  (:'org'::uuid, md5(:'org' || '20000000-0000-0000-0000-000000000010')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000002')::uuid, 'Slow page load on product listing', 'Product listing page takes 8+ seconds to load.', 'in_progress', 'high', 'web', 'agent-sarah', ARRAY['performance','ux'], md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid)
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Feedback (mix of types and ratings)
-- ============================================================

INSERT INTO feedback (org_id, id, customer_id, feedback_type, subject, body, rating, application_id, page_id) VALUES
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, 'praise', 'Love the new redesign', 'The updated product pages look great. Much easier to compare items now.', 5, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000002')::uuid, 'bug_report', 'Cart quantity glitch', 'When I change quantity to 0 and back to 1, the price shows $0.', 2, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000002')::uuid),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000003')::uuid, 'feature_request', 'Add Face ID login', 'Would be great to log in with Face ID instead of typing password every time.', 4, md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000004')::uuid),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000004')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000004')::uuid, 'complaint', 'Terrible checkout experience', 'Had to re-enter my address 3 times because the form kept clearing.', 1, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000002')::uuid),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000005')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000005')::uuid, 'suggestion', 'Add spending categories', 'It would help to see spending broken down by category on the dashboard.', 4, md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000004')::uuid),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000006')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000006')::uuid, 'praise', 'Fast shipping', 'My order arrived in 2 days. Very impressed with the speed!', 5, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, NULL),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000007')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000007')::uuid, 'bug_report', 'Balance shows wrong after transfer', 'Transferred $200 but balance only decreased by $100.', 1, md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000004')::uuid),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000008')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000008')::uuid, 'feature_request', 'Wishlist sharing', 'Let me share my wishlist with friends so they know what to get me.', 3, md5(:'org' || 'b0000000-0000-0000-0000-000000000002')::uuid, NULL),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000009')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, 'suggestion', 'Better search filters', 'Add filters for price range and brand on the search results.', 4, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000010')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000003')::uuid, 'complaint', 'App drains battery', 'The banking app uses 30% of my battery even when I am not using it.', 2, md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid, NULL),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000011')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000004')::uuid, 'praise', 'Excellent customer support', 'Agent Sarah resolved my issue within 10 minutes. Outstanding service!', 5, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, NULL),
  (:'org'::uuid, md5(:'org' || '30000000-0000-0000-0000-000000000012')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000006')::uuid, 'bug_report', 'Images not loading on product page', 'Product images show broken placeholders on Chrome 120.', 2, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || 'c0000000-0000-0000-0000-000000000001')::uuid)
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Surveys
-- ============================================================

INSERT INTO surveys (org_id, id, title, description, questions, active, application_id) VALUES
  (:'org'::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000001')::uuid, 'Post-Purchase Satisfaction', 'Tell us about your recent purchase experience.', '[
    {"id": "q1", "text": "How satisfied are you with your purchase?", "questionType": "rating", "required": true},
    {"id": "q2", "text": "How did you find our checkout process?", "questionType": "multiple_choice", "options": ["Very easy", "Easy", "Neutral", "Difficult", "Very difficult"], "required": true},
    {"id": "q3", "text": "Any additional comments?", "questionType": "text", "required": false}
  ]', true, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid),
  (:'org'::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000002')::uuid, 'App Usability Survey', 'Help us improve the mobile app experience.', '[
    {"id": "q1", "text": "How easy is it to navigate the app?", "questionType": "rating", "required": true},
    {"id": "q2", "text": "Which feature do you use most?", "questionType": "multiple_choice", "options": ["Balance check", "Transfers", "Bill pay", "Investment tracking"], "required": true},
    {"id": "q3", "text": "What feature would you like us to add?", "questionType": "text", "required": false}
  ]', true, md5(:'org' || 'b0000000-0000-0000-0000-000000000003')::uuid),
  (:'org'::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000003')::uuid, 'NPS Survey Q4 2025', 'How likely are you to recommend us?', '[
    {"id": "q1", "text": "On a scale of 1-10, how likely are you to recommend us to a friend?", "questionType": "rating", "required": true},
    {"id": "q2", "text": "What is the primary reason for your score?", "questionType": "text", "required": true}
  ]', true, NULL),
  (:'org'::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000004')::uuid, 'Website Redesign Feedback', 'We recently updated our website. Let us know what you think!', '[
    {"id": "q1", "text": "How do you rate the new design?", "questionType": "rating", "required": true},
    {"id": "q2", "text": "Is it easier to find products now?", "questionType": "multiple_choice", "options": ["Much easier", "Somewhat easier", "No change", "Harder"], "required": true},
    {"id": "q3", "text": "What do you miss from the old design?", "questionType": "text", "required": false}
  ]', false, md5(:'org' || 'b0000000-0000-0000-0000-000000000001')::uuid)
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Survey Responses
-- ============================================================

INSERT INTO survey_responses (org_id, id, survey_id, customer_id, answers, completed_at) VALUES
  (:'org'::uuid, md5(:'org' || '50000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, '[{"questionId":"q1","value":"9"},{"questionId":"q2","value":"Very easy"},{"questionId":"q3","value":"Great experience overall!"}]', now() - interval '3 days'),
  (:'org'::uuid, md5(:'org' || '50000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000002')::uuid, '[{"questionId":"q1","value":"6"},{"questionId":"q2","value":"Neutral"},{"questionId":"q3","value":"Shipping was slow."}]', now() - interval '2 days'),
  (:'org'::uuid, md5(:'org' || '50000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000005')::uuid, '[{"questionId":"q1","value":"8"},{"questionId":"q2","value":"Balance check"},{"questionId":"q3","value":"Dark mode please!"}]', now() - interval '1 day'),
  (:'org'::uuid, md5(:'org' || '50000000-0000-0000-0000-000000000004')::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000004')::uuid, '[{"questionId":"q1","value":"10"},{"questionId":"q2","value":"Amazing product quality and fast support."}]', now() - interval '5 days'),
  (:'org'::uuid, md5(:'org' || '50000000-0000-0000-0000-000000000005')::uuid, md5(:'org' || '40000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000006')::uuid, '[{"questionId":"q1","value":"4"},{"questionId":"q2","value":"Checkout is confusing and slow."}]', now() - interval '4 days')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Chat Sessions + Messages
-- ============================================================

INSERT INTO chat_sessions (org_id, id, customer_id, status) VALUES
  (:'org'::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, 'active'),
  (:'org'::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000003')::uuid, 'active')
ON CONFLICT (id) DO NOTHING;

INSERT INTO chat_messages (org_id, id, session_id, role, content, created_at) VALUES
  (:'org'::uuid, md5(:'org' || '70000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000001')::uuid, 'customer', 'Hi, I need help with my recent order.', now() - interval '10 minutes'),
  (:'org'::uuid, md5(:'org' || '70000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000001')::uuid, 'agent', 'Hello Alice! I would be happy to help. Could you share your order number?', now() - interval '9 minutes'),
  (:'org'::uuid, md5(:'org' || '70000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000001')::uuid, 'customer', 'Sure, it is ORD-2025-4821.', now() - interval '8 minutes'),
  (:'org'::uuid, md5(:'org' || '70000000-0000-0000-0000-000000000004')::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000001')::uuid, 'agent', 'I can see that order. It looks like the payment is still processing. Let me check with our payments team.', now() - interval '7 minutes'),
  (:'org'::uuid, md5(:'org' || '70000000-0000-0000-0000-000000000005')::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000002')::uuid, 'customer', 'The app keeps crashing when I try to log in.', now() - interval '5 minutes'),
  (:'org'::uuid, md5(:'org' || '70000000-0000-0000-0000-000000000006')::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000002')::uuid, 'agent', 'Sorry to hear that, Carol. What device and OS version are you using?', now() - interval '4 minutes'),
  (:'org'::uuid, md5(:'org' || '70000000-0000-0000-0000-000000000007')::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000002')::uuid, 'customer', 'iPhone 15 Pro, iOS 18.2.', now() - interval '3 minutes'),
  (:'org'::uuid, md5(:'org' || '70000000-0000-0000-0000-000000000008')::uuid, md5(:'org' || '60000000-0000-0000-0000-000000000002')::uuid, 'agent', 'Thank you. We have identified this issue and a fix will be in the next release. In the meantime, try force-closing the app and reopening.', now() - interval '2 minutes')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Interactions
-- ============================================================

INSERT INTO interactions (org_id, id, customer_id, interaction_type, metadata, created_at) VALUES
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000001')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, 'page_view', '{"page": "/products", "duration_seconds": 45}', now() - interval '2 hours'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000002')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000001')::uuid, 'purchase', '{"order_id": "ORD-2025-4821", "amount": 129.99}', now() - interval '1 hour'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000003')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000002')::uuid, 'page_view', '{"page": "/checkout", "duration_seconds": 120}', now() - interval '3 hours'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000004')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000003')::uuid, 'login', '{"method": "password", "device": "iPhone 15"}', now() - interval '30 minutes'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000005')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000004')::uuid, 'feature_use', '{"feature": "bulk_order", "items": 12}', now() - interval '4 hours'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000006')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000005')::uuid, 'support_contact', '{"channel": "phone", "reason": "password_reset"}', now() - interval '6 hours'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000007')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000006')::uuid, 'email_open', '{"campaign": "winter_sale_2025", "subject": "50% Off Winter Collection"}', now() - interval '1 day'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000008')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000006')::uuid, 'email_click', '{"campaign": "winter_sale_2025", "link": "/products?sale=winter"}', now() - interval '23 hours'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000009')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000007')::uuid, 'page_view', '{"page": "/dashboard", "duration_seconds": 90}', now() - interval '45 minutes'),
  (:'org'::uuid, md5(:'org' || '80000000-0000-0000-0000-000000000010')::uuid, md5(:'org' || '10000000-0000-0000-0000-000000000008')::uuid, 'login', '{"method": "biometric", "device": "iPad"}', now() - interval '15 minutes')
ON CONFLICT (id) DO NOTHING;

COMMIT;
