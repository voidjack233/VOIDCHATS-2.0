package main

import (
	"context"
	"database/sql"
	"errors"
	"log"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	_ "github.com/jackc/pgx/v5/stdlib"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/api"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/postgres"
)

func main() {
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" { log.Fatal("DATABASE_URL is required") }
	db, err := sql.Open("pgx", dsn)
	if err != nil { log.Fatal(err) }
	defer db.Close()
	db.SetMaxOpenConns(10)
	db.SetMaxIdleConns(5)
	db.SetConnMaxLifetime(30 * time.Minute)

	startup, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	if err := db.PingContext(startup); err != nil {
		cancel()
		log.Fatalf("database unavailable: %v", err)
	}
	cancel()

	addr := os.Getenv("LISTEN_ADDR")
	if addr == "" { addr = "127.0.0.1:8080" }
	server := &http.Server{
		Addr: addr,
		Handler: api.New(postgres.New(db)).Handler(),
		ReadHeaderTimeout: 5 * time.Second,
		ReadTimeout: 10 * time.Second,
		WriteTimeout: 15 * time.Second,
		IdleTimeout: 60 * time.Second,
		MaxHeaderBytes: 8 << 10,
	}
	stop, stopSignal := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stopSignal()
	go func() {
		<-stop.Done()
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if err := server.Shutdown(ctx); err != nil { log.Printf("shutdown: %v", err) }
	}()
	log.Printf("VOID API listening on %s (HTTP; use TLS reverse proxy outside local development)", addr)
	if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Fatal(err)
	}
}
