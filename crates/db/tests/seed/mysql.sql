CREATE TABLE pedidos (id int PRIMARY KEY, cliente varchar(50) NOT NULL, valor decimal(10,2), data date, criado datetime(3), ativo tinyint(1), nota double, hora time, dados varbinary(10), grande bigint unsigned) CHARACTER SET utf8mb4;
INSERT INTO pedidos VALUES
 (1, 'Ana', 120.50, '2026-01-15', '2026-01-15 10:30:00.123', 1, 4.5, '10:30:00', X'0102FF', 18446744073709551615),
 (2, 'Bruno', 75.00, '2026-02-01', '2026-02-01 08:00:00', 0, NULL, '-01:00:00', NULL, 5),
 (3, 'Célia', 310.99, '2026-03-20', '2026-03-20 17:45:10', 1, 3.25, '30:00:00', X'', NULL);
CREATE TABLE clientes (nome varchar(50), cidade varchar(50)) CHARACTER SET utf8mb4;
INSERT INTO clientes VALUES ('Ana', 'Belém'), ('Bruno', 'Recife'), ('Célia', 'Manaus');
CREATE VIEW pedidos_ativos AS SELECT id, cliente, valor FROM pedidos WHERE ativo = 1;
