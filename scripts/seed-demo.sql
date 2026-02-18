-- Seed script: populates the CXP database with realistic demo data
-- Run with: psql postgres://cxp:cxp_dev@localhost:5432/cxp -f scripts/seed-demo.sql

BEGIN;

-- ============================================================
-- Catalog hierarchy: Experience > Application > Page > Component
-- ============================================================

INSERT INTO experiences (id, name, description) VALUES
  ('a0000000-0000-0000-0000-000000000001', 'E-Commerce', 'Online shopping experience'),
  ('a0000000-0000-0000-0000-000000000002', 'Mobile Banking', 'Mobile banking experience')
ON CONFLICT (id) DO NOTHING;

INSERT INTO applications (id, experience_id, name, platform, description) VALUES
  ('b0000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000001', 'Web Store', 'web', 'Main e-commerce website'),
  ('b0000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000001', 'iOS Shopping App', 'ios', 'Native iOS shopping application'),
  ('b0000000-0000-0000-0000-000000000003', 'a0000000-0000-0000-0000-000000000002', 'Banking App', 'ios', 'Mobile banking iOS app')
ON CONFLICT (id) DO NOTHING;

INSERT INTO pages (id, application_id, name, path, description) VALUES
  ('c0000000-0000-0000-0000-000000000001', 'b0000000-0000-0000-0000-000000000001', 'Product Listing', '/products', 'Browse all products'),
  ('c0000000-0000-0000-0000-000000000002', 'b0000000-0000-0000-0000-000000000001', 'Checkout', '/checkout', 'Cart and payment'),
  ('c0000000-0000-0000-0000-000000000003', 'b0000000-0000-0000-0000-000000000001', 'Account Settings', '/settings', 'User profile and preferences'),
  ('c0000000-0000-0000-0000-000000000004', 'b0000000-0000-0000-0000-000000000003', 'Dashboard', '/dashboard', 'Banking dashboard')
ON CONFLICT (id) DO NOTHING;

INSERT INTO components (id, page_id, name, component_type, description) VALUES
  ('d0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-000000000001', 'Search Bar', 'input', 'Product search'),
  ('d0000000-0000-0000-0000-000000000002', 'c0000000-0000-0000-0000-000000000002', 'Payment Form', 'form', 'Credit card entry'),
  ('d0000000-0000-0000-0000-000000000003', 'c0000000-0000-0000-0000-000000000003', 'Profile Editor', 'form', 'Edit profile fields'),
  ('d0000000-0000-0000-0000-000000000004', 'c0000000-0000-0000-0000-000000000004', 'Balance Widget', 'widget', 'Account balance display')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Customers
-- ============================================================

INSERT INTO customers (id, email, name, phone, company, tags, application_id) VALUES
  ('10000000-0000-0000-0000-000000000001', 'alice.chen@example.com', 'Alice Chen', '+1-555-0101', 'Acme Corp', ARRAY['vip','enterprise'], 'b0000000-0000-0000-0000-000000000001'),
  ('10000000-0000-0000-0000-000000000002', 'bob.martinez@example.com', 'Bob Martinez', '+1-555-0102', 'Globex Inc', ARRAY['trial'], 'b0000000-0000-0000-0000-000000000001'),
  ('10000000-0000-0000-0000-000000000003', 'carol.johnson@example.com', 'Carol Johnson', '+1-555-0103', NULL, ARRAY['free-tier'], 'b0000000-0000-0000-0000-000000000002'),
  ('10000000-0000-0000-0000-000000000004', 'david.kim@example.com', 'David Kim', NULL, 'StartupXYZ', ARRAY['enterprise','beta'], 'b0000000-0000-0000-0000-000000000001'),
  ('10000000-0000-0000-0000-000000000005', 'eva.williams@example.com', 'Eva Williams', '+1-555-0105', 'Initech', ARRAY['vip'], 'b0000000-0000-0000-0000-000000000003'),
  ('10000000-0000-0000-0000-000000000006', 'frank.nguyen@example.com', 'Frank Nguyen', '+1-555-0106', NULL, ARRAY['free-tier'], 'b0000000-0000-0000-0000-000000000001'),
  ('10000000-0000-0000-0000-000000000007', 'grace.patel@example.com', 'Grace Patel', '+1-555-0107', 'TechNova', ARRAY['enterprise'], 'b0000000-0000-0000-0000-000000000003'),
  ('10000000-0000-0000-0000-000000000008', 'henry.lee@example.com', 'Henry Lee', NULL, 'ByteForge', ARRAY['trial','beta'], 'b0000000-0000-0000-0000-000000000002')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Tickets (various statuses, priorities, channels)
-- ============================================================

INSERT INTO tickets (id, customer_id, subject, description, status, priority, channel, assigned_to, tags, application_id) VALUES
  ('20000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', 'Unable to complete checkout', 'Getting a 500 error on the payment step after entering credit card details.', 'open', 'high', 'web', 'agent-sarah', ARRAY['checkout','payment'], 'b0000000-0000-0000-0000-000000000001'),
  ('20000000-0000-0000-0000-000000000002', '10000000-0000-0000-0000-000000000002', 'Wrong item shipped', 'Ordered a blue shirt (SKU-1234) but received a red one.', 'in_progress', 'medium', 'email', 'agent-mike', ARRAY['shipping','fulfillment'], 'b0000000-0000-0000-0000-000000000001'),
  ('20000000-0000-0000-0000-000000000003', '10000000-0000-0000-0000-000000000003', 'App crashes on login', 'iOS app crashes immediately after entering credentials. iPhone 15, iOS 18.', 'open', 'urgent', 'chat', NULL, ARRAY['crash','ios','login'], 'b0000000-0000-0000-0000-000000000002'),
  ('20000000-0000-0000-0000-000000000004', '10000000-0000-0000-0000-000000000004', 'Request bulk discount pricing', 'We want to order 500+ units. Looking for enterprise pricing.', 'waiting', 'low', 'email', 'agent-sarah', ARRAY['sales','enterprise'], 'b0000000-0000-0000-0000-000000000001'),
  ('20000000-0000-0000-0000-000000000005', '10000000-0000-0000-0000-000000000005', 'Cannot reset password', 'Reset email never arrives. Checked spam folder.', 'in_progress', 'high', 'phone', 'agent-mike', ARRAY['auth','email'], 'b0000000-0000-0000-0000-000000000003'),
  ('20000000-0000-0000-0000-000000000006', '10000000-0000-0000-0000-000000000001', 'Search returns irrelevant results', 'Searching for "winter jacket" shows summer clothing.', 'open', 'medium', 'web', NULL, ARRAY['search','ux'], 'b0000000-0000-0000-0000-000000000001'),
  ('20000000-0000-0000-0000-000000000007', '10000000-0000-0000-0000-000000000006', 'Promo code not working', 'SAVE20 code shows "invalid" even though the promotion is active.', 'resolved', 'medium', 'chat', 'agent-sarah', ARRAY['promo','checkout'], 'b0000000-0000-0000-0000-000000000001'),
  ('20000000-0000-0000-0000-000000000008', '10000000-0000-0000-0000-000000000007', 'Transfer failed but money deducted', 'Attempted transfer of $500 failed but balance was still deducted.', 'open', 'urgent', 'phone', 'agent-mike', ARRAY['transfer','urgent','finance'], 'b0000000-0000-0000-0000-000000000003'),
  ('20000000-0000-0000-0000-000000000009', '10000000-0000-0000-0000-000000000008', 'Feature request: dark mode', 'Would love a dark mode option in the app.', 'closed', 'low', 'web', NULL, ARRAY['feature-request','ui'], 'b0000000-0000-0000-0000-000000000002'),
  ('20000000-0000-0000-0000-000000000010', '10000000-0000-0000-0000-000000000002', 'Slow page load on product listing', 'Product listing page takes 8+ seconds to load.', 'in_progress', 'high', 'web', 'agent-sarah', ARRAY['performance','ux'], 'b0000000-0000-0000-0000-000000000001')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Feedback (mix of types and ratings)
-- ============================================================

INSERT INTO feedback (id, customer_id, feedback_type, subject, body, rating, experience, application, page, component, application_id, page_id) VALUES
  ('30000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', 'praise', 'Love the new redesign', 'The updated product pages look great. Much easier to compare items now.', 5, 'E-Commerce', 'Web Store', '/products', NULL, 'b0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-000000000001'),
  ('30000000-0000-0000-0000-000000000002', '10000000-0000-0000-0000-000000000002', 'bug_report', 'Cart quantity glitch', 'When I change quantity to 0 and back to 1, the price shows $0.', 2, 'E-Commerce', 'Web Store', '/checkout', 'Payment Form', 'b0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-000000000002'),
  ('30000000-0000-0000-0000-000000000003', '10000000-0000-0000-0000-000000000003', 'feature_request', 'Add Face ID login', 'Would be great to log in with Face ID instead of typing password every time.', 4, 'Mobile Banking', 'Banking App', '/dashboard', NULL, 'b0000000-0000-0000-0000-000000000003', 'c0000000-0000-0000-0000-000000000004'),
  ('30000000-0000-0000-0000-000000000004', '10000000-0000-0000-0000-000000000004', 'complaint', 'Terrible checkout experience', 'Had to re-enter my address 3 times because the form kept clearing.', 1, 'E-Commerce', 'Web Store', '/checkout', 'Payment Form', 'b0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-000000000002'),
  ('30000000-0000-0000-0000-000000000005', '10000000-0000-0000-0000-000000000005', 'suggestion', 'Add spending categories', 'It would help to see spending broken down by category on the dashboard.', 4, 'Mobile Banking', 'Banking App', '/dashboard', 'Balance Widget', 'b0000000-0000-0000-0000-000000000003', 'c0000000-0000-0000-0000-000000000004'),
  ('30000000-0000-0000-0000-000000000006', '10000000-0000-0000-0000-000000000006', 'praise', 'Fast shipping', 'My order arrived in 2 days. Very impressed with the speed!', 5, 'E-Commerce', 'Web Store', NULL, NULL, 'b0000000-0000-0000-0000-000000000001', NULL),
  ('30000000-0000-0000-0000-000000000007', '10000000-0000-0000-0000-000000000007', 'bug_report', 'Balance shows wrong after transfer', 'Transferred $200 but balance only decreased by $100.', 1, 'Mobile Banking', 'Banking App', '/dashboard', 'Balance Widget', 'b0000000-0000-0000-0000-000000000003', 'c0000000-0000-0000-0000-000000000004'),
  ('30000000-0000-0000-0000-000000000008', '10000000-0000-0000-0000-000000000008', 'feature_request', 'Wishlist sharing', 'Let me share my wishlist with friends so they know what to get me.', 3, 'E-Commerce', 'iOS Shopping App', NULL, NULL, 'b0000000-0000-0000-0000-000000000002', NULL),
  ('30000000-0000-0000-0000-000000000009', '10000000-0000-0000-0000-000000000001', 'suggestion', 'Better search filters', 'Add filters for price range and brand on the search results.', 4, 'E-Commerce', 'Web Store', '/products', 'Search Bar', 'b0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-000000000001'),
  ('30000000-0000-0000-0000-000000000010', '10000000-0000-0000-0000-000000000003', 'complaint', 'App drains battery', 'The banking app uses 30% of my battery even when I am not using it.', 2, 'Mobile Banking', 'Banking App', NULL, NULL, 'b0000000-0000-0000-0000-000000000003', NULL),
  ('30000000-0000-0000-0000-000000000011', '10000000-0000-0000-0000-000000000004', 'praise', 'Excellent customer support', 'Agent Sarah resolved my issue within 10 minutes. Outstanding service!', 5, 'E-Commerce', 'Web Store', NULL, NULL, 'b0000000-0000-0000-0000-000000000001', NULL),
  ('30000000-0000-0000-0000-000000000012', '10000000-0000-0000-0000-000000000006', 'bug_report', 'Images not loading on product page', 'Product images show broken placeholders on Chrome 120.', 2, 'E-Commerce', 'Web Store', '/products', NULL, 'b0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-000000000001')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Surveys
-- ============================================================

INSERT INTO surveys (id, title, description, questions, active, application_id) VALUES
  ('40000000-0000-0000-0000-000000000001', 'Post-Purchase Satisfaction', 'Tell us about your recent purchase experience.', '[
    {"id": "q1", "text": "How satisfied are you with your purchase?", "questionType": "rating", "required": true},
    {"id": "q2", "text": "How did you find our checkout process?", "questionType": "multiple_choice", "options": ["Very easy", "Easy", "Neutral", "Difficult", "Very difficult"], "required": true},
    {"id": "q3", "text": "Any additional comments?", "questionType": "text", "required": false}
  ]', true, 'b0000000-0000-0000-0000-000000000001'),
  ('40000000-0000-0000-0000-000000000002', 'App Usability Survey', 'Help us improve the mobile app experience.', '[
    {"id": "q1", "text": "How easy is it to navigate the app?", "questionType": "rating", "required": true},
    {"id": "q2", "text": "Which feature do you use most?", "questionType": "multiple_choice", "options": ["Balance check", "Transfers", "Bill pay", "Investment tracking"], "required": true},
    {"id": "q3", "text": "What feature would you like us to add?", "questionType": "text", "required": false}
  ]', true, 'b0000000-0000-0000-0000-000000000003'),
  ('40000000-0000-0000-0000-000000000003', 'NPS Survey Q4 2025', 'How likely are you to recommend us?', '[
    {"id": "q1", "text": "On a scale of 1-10, how likely are you to recommend us to a friend?", "questionType": "rating", "required": true},
    {"id": "q2", "text": "What is the primary reason for your score?", "questionType": "text", "required": true}
  ]', true, NULL),
  ('40000000-0000-0000-0000-000000000004', 'Website Redesign Feedback', 'We recently updated our website. Let us know what you think!', '[
    {"id": "q1", "text": "How do you rate the new design?", "questionType": "rating", "required": true},
    {"id": "q2", "text": "Is it easier to find products now?", "questionType": "multiple_choice", "options": ["Much easier", "Somewhat easier", "No change", "Harder"], "required": true},
    {"id": "q3", "text": "What do you miss from the old design?", "questionType": "text", "required": false}
  ]', false, 'b0000000-0000-0000-0000-000000000001')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Survey Responses
-- ============================================================

INSERT INTO survey_responses (id, survey_id, customer_id, answers, completed_at) VALUES
  ('50000000-0000-0000-0000-000000000001', '40000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', '[{"questionId":"q1","value":"9"},{"questionId":"q2","value":"Very easy"},{"questionId":"q3","value":"Great experience overall!"}]', now() - interval '3 days'),
  ('50000000-0000-0000-0000-000000000002', '40000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000002', '[{"questionId":"q1","value":"6"},{"questionId":"q2","value":"Neutral"},{"questionId":"q3","value":"Shipping was slow."}]', now() - interval '2 days'),
  ('50000000-0000-0000-0000-000000000003', '40000000-0000-0000-0000-000000000002', '10000000-0000-0000-0000-000000000005', '[{"questionId":"q1","value":"8"},{"questionId":"q2","value":"Balance check"},{"questionId":"q3","value":"Dark mode please!"}]', now() - interval '1 day'),
  ('50000000-0000-0000-0000-000000000004', '40000000-0000-0000-0000-000000000003', '10000000-0000-0000-0000-000000000004', '[{"questionId":"q1","value":"10"},{"questionId":"q2","value":"Amazing product quality and fast support."}]', now() - interval '5 days'),
  ('50000000-0000-0000-0000-000000000005', '40000000-0000-0000-0000-000000000003', '10000000-0000-0000-0000-000000000006', '[{"questionId":"q1","value":"4"},{"questionId":"q2","value":"Checkout is confusing and slow."}]', now() - interval '4 days')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Chat Sessions + Messages
-- ============================================================

INSERT INTO chat_sessions (id, customer_id, status) VALUES
  ('60000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', 'active'),
  ('60000000-0000-0000-0000-000000000002', '10000000-0000-0000-0000-000000000003', 'active')
ON CONFLICT (id) DO NOTHING;

INSERT INTO chat_messages (id, session_id, role, content, created_at) VALUES
  ('70000000-0000-0000-0000-000000000001', '60000000-0000-0000-0000-000000000001', 'customer', 'Hi, I need help with my recent order.', now() - interval '10 minutes'),
  ('70000000-0000-0000-0000-000000000002', '60000000-0000-0000-0000-000000000001', 'agent', 'Hello Alice! I would be happy to help. Could you share your order number?', now() - interval '9 minutes'),
  ('70000000-0000-0000-0000-000000000003', '60000000-0000-0000-0000-000000000001', 'customer', 'Sure, it is ORD-2025-4821.', now() - interval '8 minutes'),
  ('70000000-0000-0000-0000-000000000004', '60000000-0000-0000-0000-000000000001', 'agent', 'I can see that order. It looks like the payment is still processing. Let me check with our payments team.', now() - interval '7 minutes'),
  ('70000000-0000-0000-0000-000000000005', '60000000-0000-0000-0000-000000000002', 'customer', 'The app keeps crashing when I try to log in.', now() - interval '5 minutes'),
  ('70000000-0000-0000-0000-000000000006', '60000000-0000-0000-0000-000000000002', 'agent', 'Sorry to hear that, Carol. What device and OS version are you using?', now() - interval '4 minutes'),
  ('70000000-0000-0000-0000-000000000007', '60000000-0000-0000-0000-000000000002', 'customer', 'iPhone 15 Pro, iOS 18.2.', now() - interval '3 minutes'),
  ('70000000-0000-0000-0000-000000000008', '60000000-0000-0000-0000-000000000002', 'agent', 'Thank you. We have identified this issue and a fix will be in the next release. In the meantime, try force-closing the app and reopening.', now() - interval '2 minutes')
ON CONFLICT (id) DO NOTHING;

-- ============================================================
-- Interactions
-- ============================================================

INSERT INTO interactions (id, customer_id, interaction_type, metadata, created_at) VALUES
  ('80000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', 'page_view', '{"page": "/products", "duration_seconds": 45}', now() - interval '2 hours'),
  ('80000000-0000-0000-0000-000000000002', '10000000-0000-0000-0000-000000000001', 'purchase', '{"order_id": "ORD-2025-4821", "amount": 129.99}', now() - interval '1 hour'),
  ('80000000-0000-0000-0000-000000000003', '10000000-0000-0000-0000-000000000002', 'page_view', '{"page": "/checkout", "duration_seconds": 120}', now() - interval '3 hours'),
  ('80000000-0000-0000-0000-000000000004', '10000000-0000-0000-0000-000000000003', 'login', '{"method": "password", "device": "iPhone 15"}', now() - interval '30 minutes'),
  ('80000000-0000-0000-0000-000000000005', '10000000-0000-0000-0000-000000000004', 'feature_use', '{"feature": "bulk_order", "items": 12}', now() - interval '4 hours'),
  ('80000000-0000-0000-0000-000000000006', '10000000-0000-0000-0000-000000000005', 'support_contact', '{"channel": "phone", "reason": "password_reset"}', now() - interval '6 hours'),
  ('80000000-0000-0000-0000-000000000007', '10000000-0000-0000-0000-000000000006', 'email_open', '{"campaign": "winter_sale_2025", "subject": "50% Off Winter Collection"}', now() - interval '1 day'),
  ('80000000-0000-0000-0000-000000000008', '10000000-0000-0000-0000-000000000006', 'email_click', '{"campaign": "winter_sale_2025", "link": "/products?sale=winter"}', now() - interval '23 hours'),
  ('80000000-0000-0000-0000-000000000009', '10000000-0000-0000-0000-000000000007', 'page_view', '{"page": "/dashboard", "duration_seconds": 90}', now() - interval '45 minutes'),
  ('80000000-0000-0000-0000-000000000010', '10000000-0000-0000-0000-000000000008', 'login', '{"method": "biometric", "device": "iPad"}', now() - interval '15 minutes')
ON CONFLICT (id) DO NOTHING;

COMMIT;
