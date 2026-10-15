CREATE TABLE pedidos (id int PRIMARY KEY, cliente text NOT NULL, valor numeric(10,2), data date, criado timestamp(3), ativo boolean, nota double precision, fuso timestamptz, hora time, duracao interval, dados bytea, codigo uuid);
INSERT INTO pedidos VALUES
 (1, 'Ana', 120.50, '2026-01-15', '2026-01-15 10:30:00.123', true, 4.5, '2026-01-15 10:30:00-03', '10:30:00', '1 day 02:03:04', '\x0102ff', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'),
 (2, 'Bruno', 75.00, '2026-02-01', '2026-02-01 08:00:00', false, NULL, NULL, NULL, NULL, NULL, NULL),
 (3, 'Célia', 310.99, '2026-03-20', '2026-03-20 17:45:10', true, 3.25, '2026-03-20 17:45:10+00', '17:45:10.5', '90 minutes', '\x', NULL);
CREATE TABLE clientes (nome text, cidade text);
INSERT INTO clientes VALUES ('Ana', 'Belém'), ('Bruno', 'Recife'), ('Célia', 'Manaus');
CREATE VIEW pedidos_ativos AS SELECT id, cliente, valor FROM pedidos WHERE ativo;
CREATE SCHEMA estoque;
CREATE TABLE estoque.itens (sku varchar(10), qtd int, tags text[]);
INSERT INTO estoque.itens VALUES ('A1', 5, '{a,b}'), ('B2', 0, NULL);
