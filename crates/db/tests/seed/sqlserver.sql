CREATE DATABASE vendas;
GO
USE vendas;
GO
CREATE TABLE dbo.Pedidos (Id int PRIMARY KEY, Cliente nvarchar(50) NOT NULL, Valor decimal(10,2), Data date, Criado datetime2(3), Ativo bit, Nota float, Preco money, Fuso datetimeoffset, Hora time);
INSERT INTO dbo.Pedidos VALUES
 (1, N'Ana', 120.50, '2026-01-15', '2026-01-15 10:30:00.123', 1, 4.5, 19.99, '2026-01-15 10:30:00 -03:00', '10:30:00'),
 (2, N'Bruno', 75.00, '2026-02-01', '2026-02-01 08:00:00', 0, NULL, 5.00, NULL, NULL),
 (3, N'Célia', 310.99, '2026-03-20', '2026-03-20 17:45:10', 1, 3.25, 100.10, '2026-03-20 17:45:10 +00:00', '17:45:10.5');
CREATE TABLE dbo.Clientes (Nome nvarchar(50), Cidade nvarchar(50));
INSERT INTO dbo.Clientes VALUES (N'Ana', N'Belém'), (N'Bruno', N'Recife'), (N'Célia', N'Manaus');
GO
CREATE VIEW dbo.PedidosAtivos AS SELECT Id, Cliente, Valor FROM dbo.Pedidos WHERE Ativo = 1;
GO
CREATE SCHEMA estoque;
GO
CREATE TABLE estoque.Itens (Sku varchar(10), Qtd int);
INSERT INTO estoque.Itens VALUES ('A1', 5), ('B2', 0);
GO
