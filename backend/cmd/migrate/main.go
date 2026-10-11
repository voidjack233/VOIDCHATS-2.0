package main

import (
	"context"
	"errors"
	"log"
	"os"
	"os/signal"
	"syscall"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/config"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/database"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := run(ctx); err != nil {
		log.Print(err)
		os.Exit(1)
	}
	log.Print("database migrations applied")
}

func run(ctx context.Context) error {
	cfg, err := config.Load()
	if err != nil {
		return errors.New("invalid migration configuration")
	}
	pool, err := database.Open(ctx, cfg.Database)
	if err != nil {
		return errors.New("migration database connection failed")
	}
	defer pool.Close()
	return database.Migrate(ctx, pool)
}
